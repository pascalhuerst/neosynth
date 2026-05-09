mod audio_devices;
mod buffer;
mod callback_thread;
mod channels;
mod engine;
mod high_res_timer;
mod meters;
mod parameters;
pub mod realtime;
mod sample_format;
mod telemetry;
mod worker_thread;
mod xrun;

pub use audio_devices::*;
pub use buffer::*;
pub use channels::{
    AudioBuffer, AudioRingBufferConsumer, AudioRingBufferProducer, FRAMES_PER_BUFFER, InputChannel,
    InputParameterRingBufferConsumer, InputParameterRingBufferProducer, MAX_AUDIO_BUFFERS,
    OutputChannel, ParameterChannel, SAMPLES_PER_BUFFER, create_audio_channels,
    create_parameter_channel,
};
pub use engine::*;
pub use meters::MetersOutput;
pub use parameters::*;
pub use realtime::*;
pub use sample_format::SampleFormat;
pub use telemetry::EngineTelemetry;
pub use xrun::{XrunEventsConsumer, XrunEventsProducer, create_xrun_channel};

pub const CHANNELS: usize = 2;
