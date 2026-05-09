use super::audio_devices::{AlsaSettings, configure_audio_devices};
use super::callback_thread::{CallbackThreadConfig, start_callback_thread};
use super::channels::{InputParameterRingBufferConsumer, create_worker_audio_channels};
use super::meters::MetersOutput;
use super::sample_format::SampleFormat;
use super::telemetry::EngineTelemetry;
use super::worker_thread::start_worker_thread;
use super::xrun::XrunEventsProducer;

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

const NUM_OUTPUT_CHANNELS: usize = 2;
const DEFAULT_INPUT_CHANNELS: u32 = 2;
const DEFAULT_SAMPLE_FORMAT: SampleFormat = SampleFormat::S32Le;
/// Per audio ringbuf: enough room for two periods' worth of f32 samples.
/// One period in flight, one queued — keeps the producer from blocking under
/// normal operation while staying small enough that nothing piles up.
const AUDIO_RB_PERIODS: usize = 2;

pub struct Engine {
    input_device: String,
    output_device: String,
    sample_rate: u32,
    sample_format: SampleFormat,
    buffer_size: Option<u32>,
    period_size: Option<u32>,
    num_input_channels: u32,
}

pub struct AudioHandles {
    pub callback: JoinHandle<()>,
    pub worker: JoinHandle<()>,
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

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        running: Arc<AtomicBool>,
        ui_params: InputParameterRingBufferConsumer,
        midi_params: InputParameterRingBufferConsumer,
        osc_params: InputParameterRingBufferConsumer,
        meters: Arc<MetersOutput>,
        telemetry: Arc<EngineTelemetry>,
        xrun_producer: XrunEventsProducer,
        audio_cpu: usize,
        worker_cpu: usize,
    ) -> Result<AudioHandles> {
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

        let period_size = input_pcm.hw_params_current()?.get_period_size()? as usize;
        let buffer_size = input_pcm.hw_params_current()?.get_buffer_size()? as usize;

        let num_input_channels = self.num_input_channels as usize;

        // SPSC f32 ringbufs between callback and worker. Sizing: AUDIO_RB_PERIODS
        // periods of samples; one period fits comfortably with headroom.
        let in_capacity = AUDIO_RB_PERIODS * period_size * num_input_channels;
        let out_capacity = AUDIO_RB_PERIODS * period_size * NUM_OUTPUT_CHANNELS;
        let (in_channel, out_channel) = create_worker_audio_channels(in_capacity, out_capacity);

        let worker = start_worker_thread(
            self.sample_rate,
            period_size,
            num_input_channels,
            in_channel.consumer,
            out_channel.producer,
            ui_params,
            midi_params,
            osc_params,
            meters,
            worker_cpu,
            running.clone(),
        );

        let cb_cfg = CallbackThreadConfig {
            input_pcm,
            output_pcm,
            period_size,
            buffer_size,
            sample_rate: self.sample_rate,
            sample_format: self.sample_format,
            num_input_channels,
            audio_cpu,
        };

        let callback = start_callback_thread(
            cb_cfg,
            in_channel.producer,
            out_channel.consumer,
            telemetry,
            xrun_producer,
            running,
        );

        Ok(AudioHandles { callback, worker })
    }
}
