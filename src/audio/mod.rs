mod audio_devices;
mod buffer;
mod channels;
mod engine;
mod parameters;
pub mod realtime;

pub use audio_devices::*;
pub use buffer::*;
pub use channels::{
    AudioBuffer, AudioRingBufferConsumer, AudioRingBufferProducer, FRAMES_PER_BUFFER, InputChannel,
    InputParameterRingBufferConsumer, InputParameterRingBufferProducer, MAX_AUDIO_BUFFERS,
    OutputChannel, ParameterChannel, SAMPLES_PER_BUFFER, create_audio_channels,
    create_parameter_channel,
};
pub use engine::*;
pub use parameters::*;
pub use realtime::*;

pub const CHANNELS: usize = 2;
