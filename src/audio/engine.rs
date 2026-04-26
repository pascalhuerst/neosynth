use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::channels::InputParameterRingBufferConsumer;
use super::meters::MetersOutput;
use super::parameters::InputParameters;
use super::realtime::{prioritize_thread, set_thread_affinity};
use crate::dsp::echo::Echo;
use crate::dsp::mixer::Mixer;
use crate::dsp::reverb::Reverb;
use crate::dsp::utils::{deinterleave_and_convert_to_float, interleave_and_convert_to_i32};

use anyhow::Result;
use ringbuf::traits::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

const NUM_OUTPUT_CHANNELS: usize = 2;
const DEFAULT_INPUT_CHANNELS: u32 = 2;

pub struct Engine {
    input_device: String,
    output_device: String,
    sample_rate: u32,
    buffer_size: Option<u32>,
    period_size: Option<u32>,
    num_input_channels: u32,
}

impl Engine {
    pub fn new(input_device: String, output_device: String, sample_rate: u32) -> Self {
        Self {
            input_device,
            output_device,
            sample_rate,
            buffer_size: None,
            period_size: None,
            num_input_channels: DEFAULT_INPUT_CHANNELS,
        }
    }

    pub fn set_buffer_size(&mut self, size: u32) {
        self.buffer_size = Some(size);
    }

    pub fn set_period_size(&mut self, size: u32) {
        self.period_size = Some(size);
    }

    pub fn set_input_channels(&mut self, n: u32) {
        self.num_input_channels = n;
    }

    pub fn run(
        &self,
        running: Arc<AtomicBool>,
        params: InputParameterRingBufferConsumer,
        meters: Arc<MetersOutput>,
        audio_cpu: usize,
    ) -> Result<JoinHandle<()>> {
        let alsa_settings = AlsaSettings {
            input_device: self.input_device.clone(),
            output_device: self.output_device.clone(),
            num_input_channels: self.num_input_channels,
            num_output_channels: NUM_OUTPUT_CHANNELS as u32,
            sample_rate: self.sample_rate,
            buffer_size: self.buffer_size,
            period_size: self.period_size,
        };

        let (input_pcm, output_pcm, _) = configure_audio_devices(&alsa_settings)?;

        let capture_period = input_pcm.hw_params_current()?.get_period_size()? as usize;

        input_pcm.prepare()?;
        output_pcm.prepare()?;
        input_pcm.start()?;

        let sample_rate = self.sample_rate;
        let num_input_channels = self.num_input_channels as usize;
        let handle = std::thread::spawn(move || {
            set_thread_affinity(audio_cpu);
            prioritize_thread();
            if let Err(e) = run_audio_loop(
                input_pcm,
                output_pcm,
                capture_period,
                sample_rate,
                num_input_channels,
                params,
                meters,
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
    num_input_channels: usize,
    mut params: InputParameterRingBufferConsumer,
    meters: Arc<MetersOutput>,
    running: &AtomicBool,
) -> Result<()> {
    let input_total = period_size * num_input_channels;
    let output_total = period_size * NUM_OUTPUT_CHANNELS;

    let mut input_i32 = vec![0i32; input_total];
    let mut input_float = vec![0.0f32; input_total];
    let mut output_float = vec![0.0f32; output_total];
    let mut output_i32 = vec![0i32; output_total];
    let mut frame: Vec<f32> = vec![0.0; num_input_channels];

    let mut reverb = Reverb::new(sample_rate as f32, 1);
    let mut echo = Echo::new(sample_rate as f32, 1);
    let mut mixer = Mixer::new(sample_rate as f32, num_input_channels);

    tracing::info!(
        "Audio loop started, period_size={}, in_ch={}, out_ch={}, sample_rate={}",
        period_size,
        num_input_channels,
        NUM_OUTPUT_CHANNELS,
        sample_rate,
    );

    while running.load(Ordering::Relaxed) {
        while let Some(update) = params.try_pop() {
            match update {
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

        deinterleave_and_convert_to_float(&input_i32, &mut input_float, num_input_channels);

        mixer.reset_levels();

        for i in 0..period_size {
            for ch in 0..num_input_channels {
                frame[ch] = input_float[ch * period_size + i];
            }

            mixer.process_inputs(&frame);
            reverb.apply(mixer.reverb_bus_l, mixer.reverb_bus_r);
            echo.apply(mixer.echo_bus_l, mixer.echo_bus_r);
            mixer.add_returns(reverb.out_l, reverb.out_r, echo.out_l, echo.out_r);
            mixer.finalize();

            output_float[i] = mixer.master_l;
            output_float[period_size + i] = mixer.master_r;
        }

        // Publish peak + RMS levels (lock-free, latest-value-wins).
        let l = mixer.levels();
        let n = period_size as f32;
        let two_n = 2.0 * n;
        for (idx, (&peak, &sum_sq)) in l.input_peaks.iter().zip(l.input_sum_sq.iter()).enumerate()
        {
            meters.store_input(idx, peak, (sum_sq / n).sqrt());
        }
        meters.store_reverb(l.reverb_peak, (l.reverb_sum_sq / two_n).sqrt());
        meters.store_echo(l.echo_peak, (l.echo_sum_sq / two_n).sqrt());
        meters.store_master(
            l.master_l_peak,
            (l.master_l_sum_sq / n).sqrt(),
            l.master_r_peak,
            (l.master_r_sum_sq / n).sqrt(),
        );

        interleave_and_convert_to_i32(&output_float, &mut output_i32, NUM_OUTPUT_CHANNELS);

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
