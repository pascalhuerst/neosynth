//! Round-trip latency measurement.
//!
//! With a physical loopback cable from output channel 1 to input channel 1,
//! we send a single full-scale impulse and count how many frames pass before
//! it reappears on the input. The result is the system's round-trip latency:
//!
//!   playback_buffer + cable + capture_buffer + ALSA scheduling jitter
//!
//! No DSP or worker thread is involved — this path bypasses the engine and
//! talks directly to ALSA so the only buffering in the loop is the kernel's.
//!
//! Detection is sample-accurate within a single readi period: the input
//! buffer is scanned for the first sample whose absolute value exceeds
//! `DETECT_THRESHOLD`. If nothing is heard within `TIMEOUT_FRAMES`, the test
//! errors out (most likely cause: cable not connected, or input gain too low
//! for the output level being looped back).

use crate::audio::{
    AlsaSettings, SampleFormat, configure_audio_devices, prioritize_thread, set_thread_affinity,
};
use anyhow::{Context, Result, anyhow};
use std::io::{self, Write};

/// Periods of silence before the impulse so ALSA buffers reach steady state.
const SETTLE_PERIODS: usize = 16;
/// Maximum frames to wait for the impulse before giving up (~3 s at 48 kHz).
const TIMEOUT_FRAMES: usize = 144_000;
/// Absolute-value threshold for "this sample is the impulse coming back".
/// 0.05 ≈ -26 dBFS — well above noise on a clean cable, well below the 1.0
/// level we're injecting.
const DETECT_THRESHOLD: f32 = 0.05;

#[allow(clippy::too_many_arguments)]
pub fn run(
    input_device: String,
    output_device: String,
    sample_rate: u32,
    sample_format: SampleFormat,
    buffer_size: Option<u32>,
    period_size: Option<u32>,
    num_input_channels: u32,
    num_output_channels: u32,
    audio_cpu: usize,
) -> Result<()> {
    println!();
    println!("================================================================");
    println!(" neosynth — round-trip latency measurement");
    println!("================================================================");
    println!();
    println!(
        " Settings: rate={} Hz, format={:?}, buffer={}, period={}, in_ch={}, out_ch={}",
        sample_rate,
        sample_format,
        buffer_size.map_or("auto".into(), |b| b.to_string()),
        period_size.map_or("auto".into(), |p| p.to_string()),
        num_input_channels,
        num_output_channels,
    );
    println!();
    println!(" Connect a cable from OUTPUT channel 1 to INPUT channel 1.");
    println!(" Watch the level — set input gain so a unity-scale signal isn't");
    println!(" clipped. Headphones disconnected if this is line-level gear.");
    println!();
    print!(" Press <Enter> when ready (Ctrl+C to abort): ");
    io::stdout().flush().ok();

    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .context("reading confirmation from stdin")?;

    println!();
    println!(" Configuring ALSA…");

    let settings = AlsaSettings {
        input_device,
        output_device,
        num_input_channels,
        num_output_channels,
        sample_rate,
        sample_format,
        buffer_size,
        period_size,
    };
    let (input_pcm, output_pcm, _) = configure_audio_devices(&settings)?;

    let period_frames = input_pcm.hw_params_current()?.get_period_size()? as usize;
    let buf_frames = input_pcm.hw_params_current()?.get_buffer_size()? as usize;
    let bps = sample_format.bytes_per_sample();
    let in_ch = num_input_channels as usize;
    let out_ch = num_output_channels as usize;

    let in_period_bytes = period_frames * in_ch * bps;
    let out_period_bytes = period_frames * out_ch * bps;
    let in_period_floats = period_frames * in_ch;
    let out_period_floats = period_frames * out_ch;

    let mut in_bytes = vec![0u8; in_period_bytes];
    let mut in_floats = vec![0.0f32; in_period_floats];
    let mut out_bytes_silence = vec![0u8; out_period_bytes];
    let mut out_bytes_impulse = vec![0u8; out_period_bytes];

    // Build the impulse: deinterleaved layout, channel 0 frame 0 = 1.0.
    {
        let mut out_floats = vec![0.0f32; out_period_floats];
        out_floats[0] = 1.0;
        sample_format.encode_from_float(&out_floats, &mut out_bytes_impulse, out_ch);
        // Silence buffer (already zeros) — encode anyway for format-correct silence.
        let zero = vec![0.0f32; out_period_floats];
        sample_format.encode_from_float(&zero, &mut out_bytes_silence, out_ch);
    }

    // Pin to the chosen audio CPU + RT priority — keeps measurement clean.
    set_thread_affinity(audio_cpu);
    prioritize_thread();

    input_pcm.prepare()?;
    output_pcm.prepare()?;
    input_pcm.start()?;

    println!(
        " Period size: {} frames  ({:.2} ms)",
        period_frames,
        period_frames as f32 / sample_rate as f32 * 1000.0
    );
    println!(
        " Buffer size: {} frames  ({:.2} ms)",
        buf_frames,
        buf_frames as f32 / sample_rate as f32 * 1000.0
    );
    println!(" Settling for {} periods…", SETTLE_PERIODS);

    let mut output_frames: usize = 0;
    let mut input_frames: usize = 0;

    // Settle: write silence, drain input. Lets ALSA fill its buffers to
    // steady state so the impulse doesn't race against the cold start.
    for _ in 0..SETTLE_PERIODS {
        write_period(&output_pcm, &out_bytes_silence)?;
        output_frames += period_frames;
        read_period(&input_pcm, &mut in_bytes)?;
        input_frames += period_frames;
    }

    // Send the impulse — first sample of this period is the +1.0 click.
    println!(" Firing impulse at output frame {}", output_frames);
    let impulse_output_frame = output_frames;
    write_period(&output_pcm, &out_bytes_impulse)?;
    let _ = output_frames; // not needed after this; latency is measured in input frames

    // Listen for it on input channel 0.
    let timeout_frame = impulse_output_frame + TIMEOUT_FRAMES;
    loop {
        // Keep playback fed with silence so it doesn't underrun while we wait.
        write_period(&output_pcm, &out_bytes_silence)?;

        let frames_read = read_period(&input_pcm, &mut in_bytes)?;
        sample_format.decode_to_float(&in_bytes[..frames_read * in_ch * bps], &mut in_floats, in_ch);

        // Channel 0 occupies the first `period_frames` floats (deinterleaved).
        let ch0 = &in_floats[..period_frames.min(frames_read)];
        for (idx, &sample) in ch0.iter().enumerate() {
            if sample.abs() >= DETECT_THRESHOLD {
                let detected_at = input_frames + idx;
                let latency_frames = detected_at.saturating_sub(impulse_output_frame);
                report(latency_frames, sample_rate, period_frames, buf_frames);
                return Ok(());
            }
        }

        input_frames += frames_read;
        if input_frames > timeout_frame {
            return Err(anyhow!(
                "Timed out waiting for impulse after {} frames (~{:.1} s). \
                 Cable disconnected? Input gain too low? Threshold = {}.",
                TIMEOUT_FRAMES,
                TIMEOUT_FRAMES as f32 / sample_rate as f32,
                DETECT_THRESHOLD
            ));
        }
    }
}

fn write_period(pcm: &alsa::PCM, bytes: &[u8]) -> Result<()> {
    match pcm.io_bytes().writei(bytes) {
        Ok(_) => Ok(()),
        Err(e) if e.errno() == libc::EPIPE => {
            tracing::warn!("Playback underrun during latency test, recovering");
            pcm.try_recover(e, true).ok();
            // Don't propagate — keep measuring.
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn read_period(pcm: &alsa::PCM, bytes: &mut [u8]) -> Result<usize> {
    match pcm.io_bytes().readi(bytes) {
        Ok(frames) => Ok(frames),
        Err(e) if e.errno() == libc::EPIPE => {
            tracing::warn!("Capture overrun during latency test, recovering");
            pcm.try_recover(e, true).ok();
            Ok(0)
        }
        Err(e) => Err(e.into()),
    }
}

fn report(latency_frames: usize, sample_rate: u32, period_frames: usize, buf_frames: usize) {
    let ms = latency_frames as f32 / sample_rate as f32 * 1000.0;
    println!();
    println!("================================================================");
    println!(" Round-trip latency");
    println!("================================================================");
    println!(" Frames : {}", latency_frames);
    println!(" Time   : {:.2} ms", ms);
    println!();
    let theoretical = (2 * buf_frames) as f32 / sample_rate as f32 * 1000.0;
    println!(
        " For reference: 2 × buffer_size = {:.2} ms (theoretical lower bound",
        theoretical
    );
    println!(
        "                with full ALSA buffering — period_size={} fr).",
        period_frames
    );
    println!("================================================================");
    println!();
}
