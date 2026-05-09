use super::sample_format::SampleFormat;
use alsa::pcm::{Access, HwParams};
use alsa::{Direction, PCM};
use anyhow::{Result, anyhow};

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
    /// PCM sample format on the wire. Internal DSP is always f32.
    pub sample_format: SampleFormat,
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
    sample_format: SampleFormat,
    buffer_size: i64,
    period_size: Option<i64>,
) -> Result<()> {
    let hwp = HwParams::any(pcm)?;

    hwp.set_access(Access::RWInterleaved)?;
    hwp.set_format(sample_format.alsa_format())?;
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
        "{}: rate={}, format={:?}, channels={}, buffer={} ({:.1}ms), period={}, periods={}",
        label,
        actual_rate,
        sample_format,
        actual_channels,
        actual_buffer,
        latency_ms,
        actual_period,
        actual_buffer / actual_period
    );

    Ok(())
}

/// Configure software params for one PCM. The reference project sets these
/// explicitly to avoid relying on ALSA defaults that vary by driver:
///   * `avail_min = period_size`     — wake us per-period, not per-buffer
///   * playback `start_threshold = buffer_size` — don't begin playback until
///     the buffer is fully primed (avoids a guaranteed first underrun)
///   * capture `start_threshold  = 1` — start immediately on the first frame
fn configure_sw_params(pcm: &PCM, direction: Direction) -> Result<()> {
    let hwp = pcm.hw_params_current()?;
    let buffer_size = hwp.get_buffer_size()?;
    let period_size = hwp.get_period_size()?;

    let swp = pcm.sw_params_current()?;
    swp.set_avail_min(period_size)?;
    let start_threshold = match direction {
        Direction::Playback => buffer_size,
        Direction::Capture => 1,
    };
    swp.set_start_threshold(start_threshold)?;
    pcm.sw_params(&swp)?;
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
        settings.sample_format,
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
        settings.sample_format,
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

    // Sanity: buffer_size must be an integer multiple of period_size, else our
    // periods-per-buffer math (used for cpu-usage aggregation) breaks.
    if pb_buffer % pb_period != 0 || pb_buffer / pb_period < 1 {
        return Err(anyhow!(
            "Playback buffer_size {} is not a positive multiple of period_size {}",
            pb_buffer,
            pb_period
        ));
    }
    if cap_buffer % cap_period != 0 || cap_buffer / cap_period < 1 {
        return Err(anyhow!(
            "Capture buffer_size {} is not a positive multiple of period_size {}",
            cap_buffer,
            cap_period
        ));
    }

    configure_sw_params(&capture, Direction::Capture)?;
    configure_sw_params(&playback, Direction::Playback)?;

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
