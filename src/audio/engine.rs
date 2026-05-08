use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::channels::InputParameterRingBufferConsumer;
use super::meters::MetersOutput;
use super::parameters::InputParameters;
use super::realtime::{prioritize_thread, set_thread_affinity};
use super::sample_format::SampleFormat;
use super::telemetry::EngineTelemetry;
use crate::dsp::echo::Echo;
use crate::dsp::mixer::Mixer;
use crate::dsp::reverb::Reverb;

use anyhow::Result;
use ringbuf::traits::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

const NUM_OUTPUT_CHANNELS: usize = 2;
const DEFAULT_INPUT_CHANNELS: u32 = 2;
const DEFAULT_SAMPLE_FORMAT: SampleFormat = SampleFormat::S32Le;

pub struct Engine {
    input_device: String,
    output_device: String,
    sample_rate: u32,
    sample_format: SampleFormat,
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
            sample_format: DEFAULT_SAMPLE_FORMAT,
            buffer_size: None,
            period_size: None,
            num_input_channels: DEFAULT_INPUT_CHANNELS,
        }
    }

    pub fn set_sample_format(&mut self, fmt: SampleFormat) {
        self.sample_format = fmt;
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
        ui_params: InputParameterRingBufferConsumer,
        midi_params: InputParameterRingBufferConsumer,
        osc_params: InputParameterRingBufferConsumer,
        meters: Arc<MetersOutput>,
        telemetry: Arc<EngineTelemetry>,
        audio_cpu: usize,
    ) -> Result<JoinHandle<()>> {
        let alsa_settings = AlsaSettings {
            input_device: self.input_device.clone(),
            output_device: self.output_device.clone(),
            num_input_channels: self.num_input_channels,
            num_output_channels: NUM_OUTPUT_CHANNELS as u32,
            sample_rate: self.sample_rate,
            sample_format: self.sample_format,
            buffer_size: self.buffer_size,
            period_size: self.period_size,
        };

        let (input_pcm, output_pcm, _) = configure_audio_devices(&alsa_settings)?;

        let capture_period = input_pcm.hw_params_current()?.get_period_size()? as usize;

        input_pcm.prepare()?;
        output_pcm.prepare()?;
        input_pcm.start()?;

        let sample_rate = self.sample_rate;
        let sample_format = self.sample_format;
        let num_input_channels = self.num_input_channels as usize;
        let handle = std::thread::spawn(move || {
            set_thread_affinity(audio_cpu);
            prioritize_thread();
            if let Err(e) = run_audio_loop(
                input_pcm,
                output_pcm,
                capture_period,
                sample_rate,
                sample_format,
                num_input_channels,
                ui_params,
                midi_params,
                osc_params,
                meters,
                telemetry,
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
    sample_format: SampleFormat,
    num_input_channels: usize,
    mut ui_params: InputParameterRingBufferConsumer,
    mut midi_params: InputParameterRingBufferConsumer,
    mut osc_params: InputParameterRingBufferConsumer,
    meters: Arc<MetersOutput>,
    telemetry: Arc<EngineTelemetry>,
    running: &AtomicBool,
) -> Result<()> {
    let bps = sample_format.bytes_per_sample();
    let input_total = period_size * num_input_channels;
    let output_total = period_size * NUM_OUTPUT_CHANNELS;

    let mut input_bytes = vec![0u8; input_total * bps];
    let mut input_float = vec![0.0f32; input_total];
    let mut output_float = vec![0.0f32; output_total];
    let mut output_bytes = vec![0u8; output_total * bps];
    let mut frame: Vec<f32> = vec![0.0; num_input_channels];

    let mut reverb = Reverb::new(sample_rate as f32, 1);
    let mut echo = Echo::new(sample_rate as f32, 1);
    let mut mixer = Mixer::new(sample_rate as f32, num_input_channels);

    // Time budget per period: anything beyond this and we'll xrun.
    let period_secs = period_size as f32 / sample_rate as f32;
    // Per-iteration decay factors derived from period_secs so the EMA and
    // peak-hold time constants stay consistent across buffer sizes.
    //   EMA τ ≈ 60 ms (smooths sub-tick jitter for the readout)
    //   peak-hold τ ≈ 600 ms (slow enough that a UI tick at 30 Hz can't miss it)
    let ema_alpha = 1.0 - (-period_secs / 0.060).exp();
    let peak_decay = (-period_secs / 0.600).exp();

    let mut load_ema_pct: f32 = 0.0;
    let mut load_peak_pct: f32 = 0.0;

    tracing::info!(
        "Audio loop started, period_size={}, in_ch={}, out_ch={}, sample_rate={}, format={:?}, period_budget_us={:.1}",
        period_size,
        num_input_channels,
        NUM_OUTPUT_CHANNELS,
        sample_rate,
        sample_format,
        period_secs * 1e6,
    );

    while running.load(Ordering::Relaxed) {
        while let Some(update) = ui_params.try_pop() {
            dispatch(update, &mut reverb, &mut echo, &mut mixer);
        }
        while let Some(update) = midi_params.try_pop() {
            dispatch(update, &mut reverb, &mut echo, &mut mixer);
        }
        while let Some(update) = osc_params.try_pop() {
            dispatch(update, &mut reverb, &mut echo, &mut mixer);
        }

        match input_pcm.io_bytes().readi(&mut input_bytes) {
            Ok(_) => {}
            Err(e) if e.errno() == libc::EPIPE => {
                tracing::warn!("Capture overrun, recovering");
                input_pcm.prepare()?;
                input_pcm.start()?;
                continue;
            }
            Err(e) => return Err(e.into()),
        }

        // DSP work for one period — everything between getting the input and
        // handing the output back to ALSA. Time this region; the ratio against
        // `period_secs` is the headroom we have before we start xrunning.
        let t_work_start = Instant::now();

        sample_format.decode_to_float(&input_bytes, &mut input_float, num_input_channels);

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

        sample_format.encode_from_float(&output_float, &mut output_bytes, NUM_OUTPUT_CHANNELS);

        let work_secs = t_work_start.elapsed().as_secs_f32();
        let load_pct = work_secs / period_secs * 100.0;
        load_ema_pct += (load_pct - load_ema_pct) * ema_alpha;
        load_peak_pct = (load_peak_pct * peak_decay).max(load_pct);
        telemetry.store_dsp_load(load_ema_pct, load_peak_pct, load_pct);

        match output_pcm.io_bytes().writei(&output_bytes) {
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

#[inline]
fn dispatch(update: InputParameters, reverb: &mut Reverb, echo: &mut Echo, mixer: &mut Mixer) {
    match update {
        InputParameters::Reverb(p) => reverb.update_param(p),
        InputParameters::Echo(p) => echo.update_param(p),
        InputParameters::Mixer(p) => mixer.update_param(p),
    }
}
