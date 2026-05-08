mod audio;
mod dsp;
mod midi;
mod osc;
mod persist;
mod ui;

use anyhow::Result;
use audio::SampleFormat;
use clap::Parser;
use ringbuf::traits::Producer;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use persist::{AppState, PersistableState, default_state_path, spawn_persister};

const PARAM_CHANNEL_CAPACITY: usize = 1024;
const MIDI_CHANNEL_CAPACITY: usize = 256;
const OSC_CHANNEL_CAPACITY: usize = 256;

#[derive(Parser, Debug)]
#[command(version, about = "neosynth realtime audio engine")]
struct Cli {
    /// ALSA input (capture) device, e.g. "default", "hw:0,0", "plughw:USB"
    #[arg(long, default_value = "default")]
    input_device: String,

    /// ALSA output (playback) device
    #[arg(long, default_value = "default")]
    output_device: String,

    /// Sample rate in Hz
    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,

    /// PCM sample format on the ALSA wire. Internal DSP is always f32.
    /// Pick whichever your soundcard supports (e.g. cards with no s32le
    /// often only offer s24_3le / s24_3be).
    #[arg(long, value_enum, default_value_t = SampleFormat::S32Le)]
    sample_format: SampleFormat,

    /// Buffer size in frames (applied to both capture and playback). Smaller = lower latency,
    /// higher xrun risk.
    #[arg(long)]
    buffer_size: Option<u32>,

    /// Period size in frames. If unset, uses buffer_size / 4.
    #[arg(long)]
    period_size: Option<u32>,

    /// CPU core to pin the audio thread to (SCHED_FIFO + affinity)
    #[arg(long, default_value_t = 2)]
    audio_cpu: usize,

    /// Number of input channels to capture (one mixer strip per channel)
    #[arg(long, default_value_t = 2)]
    input_channels: u32,

    /// ALSA raw MIDI capture device, e.g. "hw:1,0,0". MIDI subsystem is
    /// skipped if not provided. Use `amidi -l` to list available devices.
    #[arg(long)]
    midi_device: Option<String>,

    /// OSC listen address (UDP), e.g. "0.0.0.0:9000". OSC subsystem is
    /// skipped if not provided. External apps can query the parameter list
    /// by sending a `/list` message; replies arrive as `/list/item` then
    /// `/list/end`.
    #[arg(long)]
    osc_listen: Option<SocketAddr>,

    /// Run headless (no UI window). The audio engine, MIDI subsystem, and
    /// state persistence still work; the process exits on Ctrl+C.
    #[arg(long, default_value_t = false)]
    no_ui: bool,
}

const PERSIST_INTERVAL: Duration = Duration::from_secs(5);

fn wait_for_shutdown(running: &AtomicBool) {
    while running.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!(
        "Starting: input={}, output={}, sample_rate={}, sample_format={:?}, in_ch={}, buffer_size={:?}, period_size={:?}, audio_cpu={}",
        cli.input_device,
        cli.output_device,
        cli.sample_rate,
        cli.sample_format,
        cli.input_channels,
        cli.buffer_size,
        cli.period_size,
        cli.audio_cpu,
    );

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        ctrlc::set_handler(move || {
            tracing::info!("Ctrl+C received, shutting down");
            running.store(false, Ordering::SeqCst);
        })?;
    }

    // ----- Persistence: load saved state, pad/truncate inputs to current count -----
    let state_path = default_state_path();
    let mut loaded_state = match &state_path {
        Some(p) => AppState::load_or_default(p),
        None => {
            tracing::warn!("Could not determine state file path; persistence disabled");
            AppState::default()
        }
    };
    loaded_state.align_inputs(cli.input_channels as usize);
    let persisted: Arc<Mutex<PersistableState>> =
        Arc::new(Mutex::new(PersistableState::new(loaded_state.clone())));

    // ----- Channels and meters -----
    let ui_params = audio::create_parameter_channel(PARAM_CHANNEL_CAPACITY);
    let midi_params = audio::create_parameter_channel(MIDI_CHANNEL_CAPACITY);
    let osc_params = audio::create_parameter_channel(OSC_CHANNEL_CAPACITY);
    let meters = Arc::new(audio::MetersOutput::new(cli.input_channels as usize));

    // ----- Engine -----
    let mut engine = audio::Engine::new(cli.input_device, cli.output_device, cli.sample_rate);
    engine.set_sample_format(cli.sample_format);
    if let Some(buf) = cli.buffer_size {
        engine.set_buffer_size(buf);
    }
    if let Some(period) = cli.period_size {
        engine.set_period_size(period);
    }
    engine.set_input_channels(cli.input_channels);

    let audio_handle = engine.run(
        running.clone(),
        ui_params.consumer,
        midi_params.consumer,
        osc_params.consumer,
        meters.clone(),
        cli.audio_cpu,
    )?;

    // Seed the audio thread with the loaded state by pushing one event per
    // parameter through the UI producer (which we then hand to the UI itself).
    let mut ui_producer = ui_params.producer;
    for ev in loaded_state.replay_events() {
        if ui_producer.try_push(ev).is_err() {
            tracing::warn!("Initial state replay: parameter channel full");
        }
    }

    // ----- MIDI -----
    let midi_handle = match cli.midi_device.clone() {
        None => {
            tracing::info!("MIDI subsystem disabled (no --midi-device)");
            None
        }
        Some(device) => match midi::run(
            running.clone(),
            midi_params.producer,
            persisted.clone(),
            device,
            cli.input_channels as usize,
        ) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("MIDI subsystem failed to start: {} (continuing without)", e);
                None
            }
        },
    };

    // ----- OSC -----
    let osc_handle = match cli.osc_listen {
        None => {
            tracing::info!("OSC subsystem disabled (no --osc-listen)");
            None
        }
        Some(addr) => match osc::run(
            running.clone(),
            osc_params.producer,
            persisted.clone(),
            addr,
            cli.input_channels as usize,
        ) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("OSC subsystem failed to start: {} (continuing without)", e);
                None
            }
        },
    };

    // ----- Persister (runs in both UI and headless modes) -----
    let persister_handle = spawn_persister(
        running.clone(),
        persisted.clone(),
        state_path.clone(),
        PERSIST_INTERVAL,
    );

    // ----- Run loop: UI window or headless wait -----
    if cli.no_ui {
        tracing::info!("Headless mode (--no-ui). Press Ctrl+C to shut down.");
        // Drop the UI producer — without UI, only MIDI feeds the audio thread.
        // The UI ringbuf consumer in the audio thread just sees nothing.
        drop(ui_producer);
        wait_for_shutdown(&running);
    } else {
        ui::run(
            ui_producer,
            running.clone(),
            cli.input_channels as usize,
            meters,
            persisted.clone(),
            loaded_state,
        )?;
        // ui::run already flips running=false on close, but make it explicit.
        running.store(false, Ordering::SeqCst);
    }

    // ----- Final save: capture anything dirty in the last debounce window -----
    if let Some(p) = &state_path {
        let guard = persisted.lock().expect("state lock");
        if let Err(e) = guard.state.save_atomic(p) {
            tracing::warn!("Final state save failed: {}", e);
        } else {
            tracing::info!("Saved state to {}", p.display());
        }
    }

    let _ = persister_handle.join();
    if let Some(h) = midi_handle {
        let _ = h.join();
    }
    if let Some(h) = osc_handle {
        let _ = h.join();
    }
    audio_handle
        .join()
        .map_err(|_| anyhow::anyhow!("audio thread panicked"))?;

    tracing::info!("Shutdown complete");
    Ok(())
}
