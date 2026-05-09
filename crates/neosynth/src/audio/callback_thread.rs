use super::channels::{SamplesRingBufferConsumer, SamplesRingBufferProducer};
use super::high_res_timer::{cpu_usage, get_ticks_in_microseconds};
use super::realtime::{prioritize_thread, set_thread_affinity};
use super::sample_format::SampleFormat;
use super::telemetry::EngineTelemetry;

use anyhow::Result;
use ringbuf::traits::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Cache prewarm cycles before we go live. Pushes zero buffers through the
/// worker so its DSP code paths are hot when capture actually starts.
const PREWARM_CYCLES: usize = 100;

pub struct CallbackThreadConfig {
    pub input_pcm: alsa::PCM,
    pub output_pcm: alsa::PCM,
    pub period_size: usize,
    pub buffer_size: usize,
    pub sample_rate: u32,
    pub sample_format: SampleFormat,
    pub num_input_channels: usize,
    pub num_output_channels: usize,
    pub audio_cpu: usize,
}

/// Spawn the audio callback thread. Owns both PCMs; pushes input frames to
/// the worker and pops output frames back. All ALSA I/O and xrun handling
/// happen here — DSP runs on a separate core.
pub fn start_callback_thread(
    cfg: CallbackThreadConfig,
    audio_in_producer: SamplesRingBufferProducer,
    audio_out_consumer: SamplesRingBufferConsumer,
    telemetry: Arc<EngineTelemetry>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        set_thread_affinity(cfg.audio_cpu);
        prioritize_thread();

        if let Err(e) = run_callback_loop(cfg, audio_in_producer, audio_out_consumer, telemetry, &running) {
            tracing::error!("Callback thread error: {}", e);
        }
        tracing::info!("Callback thread stopped");
    })
}

fn run_callback_loop(
    cfg: CallbackThreadConfig,
    mut audio_in: SamplesRingBufferProducer,
    mut audio_out: SamplesRingBufferConsumer,
    telemetry: Arc<EngineTelemetry>,
    running: &AtomicBool,
) -> Result<()> {
    let CallbackThreadConfig {
        input_pcm,
        output_pcm,
        period_size,
        buffer_size,
        sample_rate,
        sample_format,
        num_input_channels,
        num_output_channels,
        audio_cpu: _,
    } = cfg;

    let bps = sample_format.bytes_per_sample();
    let input_total = period_size * num_input_channels;
    let output_total = period_size * num_output_channels;

    let mut input_bytes = vec![0u8; input_total * bps];
    let mut input_float = vec![0.0f32; input_total];
    let mut output_float = vec![0.0f32; output_total];
    let mut output_bytes = vec![0u8; output_total * bps];

    // Buffer time in seconds: cpu_usage_buffer = average over `periods_per_buffer`.
    let periods_per_buffer = (buffer_size / period_size).max(1);
    let period_secs = period_size as f32 / sample_rate as f32;

    // Per-iteration EMA + peak-hold decay constants — reused from the previous
    // single-thread engine so the UI badge keeps the same feel.
    let ema_alpha = 1.0 - (-period_secs / 0.060).exp();
    let peak_decay = (-period_secs / 0.600).exp();
    let mut load_ema_pct: f32 = 0.0;
    let mut load_peak_pct: f32 = 0.0;
    let mut buffer_aggregate: f32 = 0.0;
    let mut callback_counter: usize = 0;

    // Preroll: drain anything stale, prepare both PCMs, then run a few hundred
    // dummy periods through the worker before we hit `capture.start()`. This
    // primes the i-cache and any branch-predictor state on the worker core.
    output_pcm.drain().ok();
    output_pcm.prepare()?;
    input_pcm.prepare()?;

    tracing::info!(
        "Callback prewarm: {} cycles before capture.start()",
        PREWARM_CYCLES
    );
    let zeros = vec![0.0f32; input_total];
    for _ in 0..PREWARM_CYCLES {
        if !running.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Spin for vacant space, push zeros, then pull the worker's output.
        while audio_in.vacant_len() < input_total {
            if !running.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::hint::spin_loop();
        }
        let _ = audio_in.push_slice(&zeros);

        while audio_out.occupied_len() < output_total {
            if !running.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::hint::spin_loop();
        }
        let _ = audio_out.pop_slice(&mut output_float);
    }

    tracing::info!(
        "Callback loop started, period_size={}, buffer_size={}, periods_per_buffer={}, format={:?}, period_budget_us={:.1}",
        period_size,
        buffer_size,
        periods_per_buffer,
        sample_format,
        period_secs * 1e6,
    );

    input_pcm.start()?;

    while running.load(Ordering::Relaxed) {
        match input_pcm.io_bytes().readi(&mut input_bytes) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Capture overrun, recovering");
                telemetry.increment_overrun();
                if input_pcm.try_recover(e, true).is_err() {
                    input_pcm.prepare()?;
                    input_pcm.start()?;
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }

        // Time-budget window: from after the input is in our hands, through
        // pushing to the worker, waiting for it, and encoding for ALSA. This
        // is the elapsed wall-clock time we needed for one period.
        let t_work_start = get_ticks_in_microseconds();

        sample_format.decode_to_float(&input_bytes, &mut input_float, num_input_channels);

        // Hand input to the worker.
        while audio_in.vacant_len() < input_total {
            if !running.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::hint::spin_loop();
        }
        let pushed = audio_in.push_slice(&input_float);
        debug_assert_eq!(pushed, input_total);

        // Wait for the worker's output for this period.
        while audio_out.occupied_len() < output_total {
            if !running.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::hint::spin_loop();
        }
        let popped = audio_out.pop_slice(&mut output_float);
        debug_assert_eq!(popped, output_total);

        sample_format.encode_from_float(&output_float, &mut output_bytes, num_output_channels);

        // Accumulate this period's load fraction; emit once per buffer.
        let cpu_usage_period = cpu_usage(t_work_start, period_size, sample_rate as usize);
        buffer_aggregate += cpu_usage_period;
        callback_counter += 1;
        if callback_counter >= periods_per_buffer {
            let buffer_avg_pct = buffer_aggregate / periods_per_buffer as f32 * 100.0;
            buffer_aggregate = 0.0;
            callback_counter = 0;

            load_ema_pct += (buffer_avg_pct - load_ema_pct) * ema_alpha;
            load_peak_pct = (load_peak_pct * peak_decay).max(buffer_avg_pct);
            telemetry.store_dsp_load(load_ema_pct, load_peak_pct, buffer_avg_pct);
        }

        match output_pcm.io_bytes().writei(&output_bytes) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Playback underrun, recovering");
                telemetry.increment_underrun();
                if output_pcm.try_recover(e, true).is_err() {
                    output_pcm.drain().ok();
                    output_pcm.prepare()?;
                }
                // Re-issue the period so we don't drop a buffer's worth of audio.
                let _ = output_pcm.io_bytes().writei(&output_bytes);
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
