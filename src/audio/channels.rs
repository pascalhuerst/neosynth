use super::buffer::GenericAudioBuffer;
use super::parameters::InputParameters;
use ringbuf::{HeapRb, StaticRb, traits::*};

pub const MAX_AUDIO_BUFFERS: usize = 4;
pub const CHANNELS: usize = 8;
pub const FRAMES_PER_BUFFER: usize = 4096;
pub const SAMPLES_PER_BUFFER: usize = CHANNELS * FRAMES_PER_BUFFER;

pub type AudioBuffer = GenericAudioBuffer<i16, SAMPLES_PER_BUFFER>;

type AudioRingBuffer = StaticRb<AudioBuffer, MAX_AUDIO_BUFFERS>;
pub type AudioRingBufferConsumer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<AudioRingBuffer>, false, true>;
pub type AudioRingBufferProducer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<AudioRingBuffer>, true, false>;

pub struct InputChannel {
    pub input_producer: AudioRingBufferProducer,
    pub input_consumer: AudioRingBufferConsumer,
}

pub struct OutputChannel {
    pub output_producer: AudioRingBufferProducer,
    pub output_consumer: AudioRingBufferConsumer,
}

pub fn create_audio_channels() -> (InputChannel, OutputChannel) {
    let rb_input = AudioRingBuffer::default();
    let rb_output = AudioRingBuffer::default();
    let (input_producer, input_consumer) = rb_input.split();
    let (output_producer, output_consumer) = rb_output.split();
    (
        InputChannel {
            input_producer,
            input_consumer,
        },
        OutputChannel {
            output_consumer,
            output_producer,
        },
    )
}

pub type InputParameterRingBuffer = HeapRb<InputParameters>;
pub type InputParameterRingBufferConsumer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<InputParameterRingBuffer>, false, true>;
pub type InputParameterRingBufferProducer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<InputParameterRingBuffer>, true, false>;

pub struct ParameterChannel {
    pub producer: InputParameterRingBufferProducer,
    pub consumer: InputParameterRingBufferConsumer,
}

pub fn create_parameter_channel(capacity: usize) -> ParameterChannel {
    let rb = InputParameterRingBuffer::new(capacity);
    let (producer, consumer) = rb.split();
    ParameterChannel { producer, consumer }
}
