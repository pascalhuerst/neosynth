mod audio_devices;
mod buffer;
mod channels;
mod engine;
mod meters;
mod parameters;
pub mod realtime;
mod sample_format;
mod telemetry;

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

pub const CHANNELS: usize = 2;
