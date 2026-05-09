use std::sync::atomic::{AtomicU32, Ordering};

/// Lock-free, latest-value-wins channel for engine→UI telemetry.
///
/// Same shape as `MetersOutput`: the audio thread stores; the UI reads at its
/// own rate and missed updates are simply overwritten. No allocation, no
/// blocking, safe to call from the realtime path.
///
/// First field: DSP load. Computed in the engine as
///   load = (callback_processing_time / period_duration) * 100
/// where `period_duration = period_size / sample_rate`. Crossing 100 % means
/// the audio thread can't keep up and xruns become inevitable.
///
/// Two values are published so the UI doesn't miss bursts between its ticks
/// (the UI runs at ~30 Hz; at period_size=128 / 48 kHz the audio loop fires at
/// ~370 Hz):
///   * `dsp_load_pct`      — short EMA-smoothed instantaneous load
///   * `dsp_load_peak_pct` — slowly decaying peak-hold; better for a status
///                            readout because momentary spikes don't get lost
pub struct EngineTelemetry {
    dsp_load_pct: AtomicU32,
    dsp_load_peak_pct: AtomicU32,
    /// True running max since the last `take_dsp_load_max_pct()`. Used by the
    /// headless logger to report a real window max, not a decaying peak.
    /// Non-positive values are treated as "unset" (only finite, non-negative
    /// numbers are stored, so the 0-bit pattern doubles as the empty marker).
    dsp_load_max_pct: AtomicU32,
}

impl EngineTelemetry {
    pub fn new() -> Self {
        Self {
            dsp_load_pct: AtomicU32::new(0),
            dsp_load_peak_pct: AtomicU32::new(0),
            dsp_load_max_pct: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn store_dsp_load(&self, ema_pct: f32, peak_pct: f32, raw_pct: f32) {
        self.dsp_load_pct
            .store(ema_pct.to_bits(), Ordering::Relaxed);
        self.dsp_load_peak_pct
            .store(peak_pct.to_bits(), Ordering::Relaxed);
        // Monotonic max via CAS on the f32 bits. Contention is single-writer
        // (audio thread) vs occasional reset by the consumer, so the loop
        // bounds out in 1–2 iterations on a hot path.
        let bits = raw_pct.to_bits();
        let mut cur = self.dsp_load_max_pct.load(Ordering::Relaxed);
        while f32::from_bits(cur) < raw_pct {
            match self.dsp_load_max_pct.compare_exchange_weak(
                cur,
                bits,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }

    #[inline]
    pub fn dsp_load_peak_pct(&self) -> f32 {
        f32::from_bits(self.dsp_load_peak_pct.load(Ordering::Relaxed))
    }

    /// Atomically read the running max since the last call and reset to zero.
    /// Use this for periodic windowed logging.
    #[inline]
    pub fn take_dsp_load_max_pct(&self) -> f32 {
        f32::from_bits(self.dsp_load_max_pct.swap(0, Ordering::Relaxed))
    }
}

impl Default for EngineTelemetry {
    fn default() -> Self {
        Self::new()
    }
}
