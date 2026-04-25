mod audio_devices;
mod buffer;
mod channels;
mod engine;
pub mod realtime;

pub use audio_devices::*;
pub use buffer::*;
pub use channels::{
    AudioBuffer, AudioRingBufferConsumer, AudioRingBufferProducer, FRAMES_PER_BUFFER, InputChannel,
    MAX_AUDIO_BUFFERS, OutputChannel, SAMPLES_PER_BUFFER, create_audio_channels,
};
pub use engine::*;
pub use realtime::*;

pub const CHANNELS: usize = 2;
