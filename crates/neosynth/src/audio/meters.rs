use std::sync::atomic::{AtomicU32, Ordering};

/// One meter cell holding the latest peak + RMS.
struct MeterCell {
    peak: AtomicU32,
    rms: AtomicU32,
}

impl MeterCell {
    fn new() -> Self {
        Self {
            peak: AtomicU32::new(0),
            rms: AtomicU32::new(0),
        }
    }

    #[inline]
    fn store(&self, peak: f32, rms: f32) {
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.rms.store(rms.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    fn load(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak.load(Ordering::Relaxed)),
            f32::from_bits(self.rms.load(Ordering::Relaxed)),
        )
    }
}

/// Lock-free, latest-value-wins meter publication.
///
/// The audio thread calls `store_*(peak, rms)` once per buffer with the
/// buffer's measurements. The UI thread calls `load_*` at its own rate.
/// Stores never block — if the UI never reads, old values are simply
/// overwritten.
pub struct MetersOutput {
    inputs: Vec<MeterCell>,
    reverb: MeterCell,
    echo: MeterCell,
    master_l: MeterCell,
    master_r: MeterCell,
    /// Master compressor peak gain reduction in dB (always ≥ 0). Single
    /// value (no peak/rms split) so it gets a bare atomic, not a `MeterCell`.
    compressor_gr_db: AtomicU32,
}

impl MetersOutput {
    pub fn new(num_inputs: usize) -> Self {
        Self {
            inputs: (0..num_inputs).map(|_| MeterCell::new()).collect(),
            reverb: MeterCell::new(),
            echo: MeterCell::new(),
            master_l: MeterCell::new(),
            master_r: MeterCell::new(),
            compressor_gr_db: AtomicU32::new(0),
        }
    }

    pub fn num_inputs(&self) -> usize {
        self.inputs.len()
    }

    #[inline]
    pub fn store_compressor_gr_db(&self, gr_db: f32) {
        self.compressor_gr_db
            .store(gr_db.to_bits(), Ordering::Relaxed);
    }

    #[inline]
    pub fn load_compressor_gr_db(&self) -> f32 {
        f32::from_bits(self.compressor_gr_db.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn store_input(&self, idx: usize, peak: f32, rms: f32) {
        if let Some(cell) = self.inputs.get(idx) {
            cell.store(peak, rms);
        }
    }

    #[inline]
    pub fn load_input(&self, idx: usize) -> (f32, f32) {
        self.inputs.get(idx).map(|c| c.load()).unwrap_or((0.0, 0.0))
    }

    #[inline]
    pub fn store_reverb(&self, peak: f32, rms: f32) {
        self.reverb.store(peak, rms);
    }

    #[inline]
    pub fn load_reverb(&self) -> (f32, f32) {
        self.reverb.load()
    }

    #[inline]
    pub fn store_echo(&self, peak: f32, rms: f32) {
        self.echo.store(peak, rms);
    }

    #[inline]
    pub fn load_echo(&self) -> (f32, f32) {
        self.echo.load()
    }

    #[inline]
    pub fn store_master(&self, peak_l: f32, rms_l: f32, peak_r: f32, rms_r: f32) {
        self.master_l.store(peak_l, rms_l);
        self.master_r.store(peak_r, rms_r);
    }

    #[inline]
    pub fn load_master(&self) -> ((f32, f32), (f32, f32)) {
        (self.master_l.load(), self.master_r.load())
    }
}
