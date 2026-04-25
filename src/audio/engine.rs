use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::channels::InputParameterRingBufferConsumer;
use super::parameters::InputParameters;
use super::realtime::{prioritize_thread, set_thread_affinity};
use crate::dsp::echo::Echo;
use crate::dsp::output_mixer::ChannelMixer;
use crate::dsp::processing::process_linear_gain;
use crate::dsp::reverb::Reverb;
use crate::dsp::utils::{deinterleave_and_convert_to_float, interleave_and_convert_to_i32};

use anyhow::Result;
use ringbuf::traits::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

const NUM_CHANNELS: usize = 2;

pub struct Engine {
    input_device: String,
    output_device: String,
    sample_rate: u32,
    buffer_size: Option<u32>,
}

impl Engine {
    pub fn new(input_device: String, output_device: String, sample_rate: u32) -> Self {
        Self {
            input_device,
            output_device,
            sample_rate,
            buffer_size: None,
        }
    }

    pub fn set_buffer_size(&mut self, size: u32) {
        self.buffer_size = Some(size);
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
            num_input_channels: NUM_CHANNELS as u32,
            num_output_channels: NUM_CHANNELS as u32,
            sample_rate: self.sample_rate,
            buffer_size: self.buffer_size,
        };

        let (input_pcm, output_pcm, _) = configure_audio_devices(&alsa_settings)?;

        let capture_period = input_pcm.hw_params_current()?.get_period_size()? as usize;

        input_pcm.prepare()?;
        output_pcm.prepare()?;
        input_pcm.start()?;

        let sample_rate = self.sample_rate;
        let handle = std::thread::spawn(move || {
            set_thread_affinity(audio_cpu);
            prioritize_thread();
            if let Err(e) = run_audio_loop(
                input_pcm,
                output_pcm,
                capture_period,
                sample_rate,
                params,
                &running,
            ) {
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
    sample_rate: u32,
    mut params: InputParameterRingBufferConsumer,
    running: &AtomicBool,
) -> Result<()> {
    let total_samples = period_size * NUM_CHANNELS;
    let mut input_i32 = vec![0i32; total_samples];
    let mut float_buf = vec![0.0f32; total_samples];
    let mut output_i32 = vec![0i32; total_samples];

    let mut gain: f32 = 1.0;
    let mut reverb = Reverb::new(sample_rate as f32, 1);
    let mut echo = Echo::new(sample_rate as f32, 1);
    let mut mixer = ChannelMixer::new(sample_rate as f32);

    tracing::info!(
        "Audio loop started, period_size={}, channels={}, sample_rate={}, gain={}",
        period_size,
        NUM_CHANNELS,
        sample_rate,
        gain
    );

    while running.load(Ordering::Relaxed) {
        while let Some(update) = params.try_pop() {
            match update {
                InputParameters::LinearGain(v) => gain = v as f32,
                InputParameters::Reverb(p) => reverb.update_param(p),
                InputParameters::Echo(p) => echo.update_param(p),
                InputParameters::Mixer(p) => mixer.update_param(p),
            }
        }

        match input_pcm.io_i32()?.readi(&mut input_i32) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Capture overrun, recovering");
                input_pcm.prepare()?;
                input_pcm.start()?;
                continue;
            }
            Err(e) => return Err(e.into()),
        }

        deinterleave_and_convert_to_float(&input_i32, &mut float_buf, NUM_CHANNELS);

        let buffer_size = float_buf.len() / NUM_CHANNELS;
        let (left, right) = float_buf.split_at_mut(buffer_size);
        for i in 0..buffer_size {
            let raw_l = left[i];
            let raw_r = right[i];
            reverb.apply(raw_l, raw_r);
            echo.apply(raw_l, raw_r);
            mixer.combine(
                raw_l,
                raw_r,
                reverb.out_l,
                reverb.out_r,
                echo.out_l,
                echo.out_r,
            );
            left[i] = mixer.out_l;
            right[i] = mixer.out_r;
        }

        process_linear_gain(&mut float_buf, gain);

        interleave_and_convert_to_i32(&float_buf, &mut output_i32, NUM_CHANNELS);

        match output_pcm.io_i32()?.writei(&output_i32) {
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
