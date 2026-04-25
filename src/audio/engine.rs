use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::realtime::{prioritize_thread, set_thread_affinity};

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub struct Engine {
    input_device: String,
    output_device: String,
    sample_rate: u32,
    buffer_size: Option<u32>,
    capture_buffer_size: Option<u32>,
}

impl Engine {
    pub fn new(input_device: String, output_device: String, sample_rate: u32) -> Self {
        Self {
            input_device,
            output_device,
            sample_rate,
            buffer_size: None,
            capture_buffer_size: None,
        }
    }

    pub fn set_buffer_size(&mut self, size: u32) {
        self.buffer_size = Some(size);
    }

    pub fn set_capture_buffer_size(&mut self, size: u32) {
        self.capture_buffer_size = Some(size);
    }

    pub fn run(
        &self,
        running: Arc<AtomicBool>,
        capture_cpu: usize,
        playback_cpu: usize,
    ) -> Result<(JoinHandle<()>, JoinHandle<()>)> {
        let alsa_settings = AlsaSettings {
            input_device: self.input_device.clone(),
            output_device: self.output_device.clone(),
            num_input_channels: 2,
            num_output_channels: 2,
            sample_rate: self.sample_rate,
            buffer_size: self.buffer_size,
            capture_buffer_size: self.capture_buffer_size,
            period_size: None,
        };

        let (input_pcm, output_pcm, _) = configure_audio_devices(&alsa_settings)?;

        let capture_period = input_pcm.hw_params_current()?.get_period_size()? as usize;
        let playback_period = output_pcm.hw_params_current()?.get_period_size()? as usize;

        input_pcm.prepare()?;
        output_pcm.prepare()?;
        input_pcm.start()?;

        let cap_running = running.clone();
        let capture_handle = std::thread::spawn(move || {
            set_thread_affinity(capture_cpu);
            prioritize_thread();
            if let Err(e) = run_capture_loop(input_pcm, capture_period, &cap_running) {
                tracing::error!("Capture thread error: {}", e);
            }
            tracing::info!("Capture thread stopped");
        });

        let pb_running = running;
        let playback_handle = std::thread::spawn(move || {
            set_thread_affinity(playback_cpu);
            prioritize_thread();
            if let Err(e) = run_playback_loop(output_pcm, playback_period, &pb_running) {
                tracing::error!("Playback thread error: {}", e);
            }
            tracing::info!("Playback thread stopped");
        });

        Ok((capture_handle, playback_handle))
    }
}

fn run_capture_loop(
    input_pcm: alsa::PCM,
    period_size: usize,
    running: &AtomicBool,
) -> Result<()> {
    let mut capture_buf = vec![0i16; period_size * 2];

    tracing::info!("Capture loop started, period_size={}", period_size);

    while running.load(Ordering::Relaxed) {
        match input_pcm.io_i16()?.readi(&mut capture_buf) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Capture overrun, recovering");
                input_pcm.prepare()?;
                input_pcm.start()?;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}

fn run_playback_loop(
    output_pcm: alsa::PCM,
    period_size: usize,
    running: &AtomicBool,
) -> Result<()> {
    let playback_buf = vec![0i16; period_size * 2];

    tracing::info!("Playback loop started, period_size={}", period_size);

    while running.load(Ordering::Relaxed) {
        match output_pcm.io_i16()?.writei(&playback_buf) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Playback underrun, recovering");
                output_pcm.prepare()?;
            }
            Err(e) => return Err(e.into()),
        }
    }

    Ok(())
}
