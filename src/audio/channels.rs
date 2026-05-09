use super::parameters::InputParameters;
use ringbuf::{HeapRb, traits::*};

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

// SPSC f32 sample ringbuf used between the callback and worker threads. One
// channel for callback → worker (input), one for worker → callback (output).
// Push/pop happen one period at a time via `push_slice`/`pop_slice`.
pub type SamplesRingBuffer = HeapRb<f32>;
pub type SamplesRingBufferProducer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<SamplesRingBuffer>, true, false>;
pub type SamplesRingBufferConsumer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<SamplesRingBuffer>, false, true>;

pub struct WorkerInputChannel {
    pub producer: SamplesRingBufferProducer,
    pub consumer: SamplesRingBufferConsumer,
}

pub struct WorkerOutputChannel {
    pub producer: SamplesRingBufferProducer,
    pub consumer: SamplesRingBufferConsumer,
}

pub fn create_worker_audio_channels(
    input_capacity: usize,
    output_capacity: usize,
) -> (WorkerInputChannel, WorkerOutputChannel) {
    let rb_in = SamplesRingBuffer::new(input_capacity);
    let (in_p, in_c) = rb_in.split();
    let rb_out = SamplesRingBuffer::new(output_capacity);
    let (out_p, out_c) = rb_out.split();
    (
        WorkerInputChannel {
            producer: in_p,
            consumer: in_c,
        },
        WorkerOutputChannel {
            producer: out_p,
            consumer: out_c,
        },
    )
}
