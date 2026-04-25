use alsa::pcm::{Access, Format, HwParams};
use alsa::{Direction, PCM};
use anyhow::Result;

/// Default buffer size in frames, applied to both capture and playback.
/// 512 frames at 48kHz = ~10.7ms per side.
const DEFAULT_BUFFER: i64 = 512;

/// Target number of periods per buffer. 4 is the standard sweet spot:
/// frequent enough wakeups without excessive interrupt overhead.
const TARGET_PERIODS: i64 = 4;

#[derive(Clone)]
pub struct AlsaSettings {
    pub input_device: String,
    pub output_device: String,
    pub num_input_channels: u32,
    pub num_output_channels: u32,
    pub sample_rate: u32,
    /// Buffer size in frames, used for both capture and playback (default: 512).
    pub buffer_size: Option<u32>,
    /// Period size in frames; if `None`, uses `buffer_size / TARGET_PERIODS`.
    pub period_size: Option<u32>,
}

/// Configure a PCM device with the given buffer size.
fn configure_pcm(
    pcm: &PCM,
    label: &str,
    num_channels: u32,
    sample_rate: u32,
    buffer_size: i64,
    period_size: Option<i64>,
) -> Result<()> {
    let hwp = HwParams::any(pcm)?;

    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(Format::s32())?;
    hwp.set_channels(num_channels)?;

    // Disable ALSA's software resampling — we do our own pitch-based resampling
    hwp.set_rate_resample(false)?;

    hwp.set_rate(sample_rate, alsa::ValueOr::Nearest)?;

    hwp.set_buffer_size_near(buffer_size)?;

    let period = period_size.unwrap_or_else(|| (buffer_size / TARGET_PERIODS).max(2));
    hwp.set_period_size_near(period, alsa::ValueOr::Nearest)?;

    pcm.hw_params(&hwp)?;

    let actual_rate = hwp.get_rate()?;
    let actual_channels = hwp.get_channels()?;
    let actual_buffer = hwp.get_buffer_size()?;
    let actual_period = hwp.get_period_size()?;
    let latency_ms = actual_buffer as f64 / actual_rate as f64 * 1000.0;

    tracing::info!(
        "{}: rate={}, channels={}, buffer={} ({:.1}ms), period={}, periods={}",
        label,
        actual_rate,
        actual_channels,
        actual_buffer,
        latency_ms,
        actual_period,
        actual_buffer / actual_period
    );

    Ok(())
}

pub fn configure_audio_devices(settings: &AlsaSettings) -> Result<(PCM, PCM, usize)> {
    let target_buffer = settings
        .buffer_size
        .map(|b| b as i64)
        .unwrap_or(DEFAULT_BUFFER);
    let target_period = settings.period_size.map(|p| p as i64);

    tracing::info!("Configuring capture: {}", settings.input_device);
    let capture = PCM::new(&settings.input_device, Direction::Capture, false)?;
    configure_pcm(
        &capture,
        "Capture",
        settings.num_input_channels,
        settings.sample_rate,
        target_buffer,
        target_period,
    )?;

    tracing::info!("Configuring playback: {}", settings.output_device);
    let playback = PCM::new(&settings.output_device, Direction::Playback, false)?;
    configure_pcm(
        &playback,
        "Playback",
        settings.num_output_channels,
        settings.sample_rate,
        target_buffer,
        target_period,
    )?;

    let cap_buffer = capture.hw_params_current()?.get_buffer_size()?;
    let pb_buffer = playback.hw_params_current()?.get_buffer_size()?;
    let cap_period = capture.hw_params_current()?.get_period_size()?;
    let pb_period = playback.hw_params_current()?.get_period_size()?;

    if cap_buffer != pb_buffer || cap_period != pb_period {
        tracing::warn!(
            "Capture and playback negotiated different sizes (buffer {} vs {}, period {} vs {}); \
             ALSA picked the nearest supported value for each device",
            cap_buffer,
            pb_buffer,
            cap_period,
            pb_period
        );
    }

    let total_latency_ms =
        (cap_buffer + pb_buffer) as f64 / settings.sample_rate as f64 * 1000.0;
    tracing::info!(
        "Expected overall latency: {:.1}ms (capture {} + playback {} frames @ {}Hz)",
        total_latency_ms,
        cap_buffer,
        pb_buffer,
        settings.sample_rate
    );

    Ok((capture, playback, cap_buffer as usize))
}
