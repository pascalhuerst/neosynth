mod audio;
mod dsp;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PARAM_CHANNEL_CAPACITY: usize = 1024;

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
        "Starting: input={}, output={}, sample_rate={}, in_ch={}, buffer_size={:?}, period_size={:?}, audio_cpu={}",
        cli.input_device,
        cli.output_device,
        cli.sample_rate,
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

    let params = audio::create_parameter_channel(PARAM_CHANNEL_CAPACITY);
    let meters = Arc::new(audio::MetersOutput::new(cli.input_channels as usize));

    let mut engine = audio::Engine::new(cli.input_device, cli.output_device, cli.sample_rate);
    if let Some(buf) = cli.buffer_size {
        engine.set_buffer_size(buf);
    }
    if let Some(period) = cli.period_size {
        engine.set_period_size(period);
    }
    engine.set_input_channels(cli.input_channels);

    let audio_handle = engine.run(
        running.clone(),
        params.consumer,
        meters.clone(),
        cli.audio_cpu,
    )?;

    ui::run(
        params.producer,
        running.clone(),
        cli.input_channels as usize,
        meters,
    )?;

    audio_handle
        .join()
        .map_err(|_| anyhow::anyhow!("audio thread panicked"))?;

    tracing::info!("Shutdown complete");
    Ok(())
}
