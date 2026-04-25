use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::channels::InputParameterRingBufferConsumer;
use super::parameters::InputParameters;
use super::realtime::{prioritize_thread, set_thread_affinity};

use anyhow::Result;
use ringbuf::traits::*;
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
        params: InputParameterRingBufferConsumer,
        audio_cpu: usize,
    ) -> Result<JoinHandle<()>> {
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

        input_pcm.prepare()?;
        output_pcm.prepare()?;
        input_pcm.start()?;

        let handle = std::thread::spawn(move || {
            set_thread_affinity(audio_cpu);
            prioritize_thread();
            if let Err(e) = run_audio_loop(input_pcm, output_pcm, capture_period, params, &running)
            {
                tracing::error!("Audio thread error: {}", e);
            }
            tracing::info!("Audio thread stopped");
        });

        Ok(handle)
    }
}

fn run_audio_loop(
    input_pcm: alsa::PCM,
    output_pcm: alsa::PCM,
    period_size: usize,
    mut params: InputParameterRingBufferConsumer,
    running: &AtomicBool,
) -> Result<()> {
    let mut buf = vec![0i16; period_size * 2];
    let mut gain: f32 = 1.0;

    tracing::info!(
        "Audio loop started, period_size={}, gain={}",
        period_size,
        gain
    );

    while running.load(Ordering::Relaxed) {
        while let Some(update) = params.try_pop() {
            match update {
                InputParameters::LinearGain(g) => {
                    gain = g as f32;
                    tracing::info!("Linear gain set to {}", gain);
                }
            }
        }

        match input_pcm.io_i16()?.readi(&mut buf) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Capture overrun, recovering");
                input_pcm.prepare()?;
                input_pcm.start()?;
                continue;
            }
            Err(e) => return Err(e.into()),
        }

        if gain != 1.0 {
            for s in buf.iter_mut() {
                let v = (*s as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32);
                *s = v as i16;
            }
        }

        match output_pcm.io_i16()?.writei(&buf) {
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
