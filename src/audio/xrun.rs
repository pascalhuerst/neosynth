use ringbuf::{HeapRb, traits::Split};

/// Discrete xrun event delivered from the callback thread to the UI / logger.
/// Atomic snapshot metering can't deliver these without loss — they need a
/// queued channel.
#[derive(Debug, Clone, Copy)]
pub enum XrunKind {
    /// Capture overrun — the kernel filled the input buffer before we read it.
    Overrun,
    /// Playback underrun — the kernel emptied the output buffer before we
    /// wrote the next period.
    Underrun,
}

#[derive(Debug, Clone, Copy)]
pub struct XrunEvent {
    pub kind: XrunKind,
    /// Microseconds since CLOCK_MONOTONIC epoch (matches `high_res_timer`).
    pub timestamp_us: i64,
}

pub type XrunEventsRb = HeapRb<XrunEvent>;
pub type XrunEventsProducer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<XrunEventsRb>, true, false>;
pub type XrunEventsConsumer =
    ringbuf::wrap::caching::Caching<std::sync::Arc<XrunEventsRb>, false, true>;

pub struct XrunEventsChannel {
    pub producer: XrunEventsProducer,
    pub consumer: XrunEventsConsumer,
}

pub fn create_xrun_channel(capacity: usize) -> XrunEventsChannel {
    let rb = XrunEventsRb::new(capacity);
    let (producer, consumer) = rb.split();
    XrunEventsChannel { producer, consumer }
}
