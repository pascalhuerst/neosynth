/// Map a normalized 0..1 control-surface value (MIDI 0..127, UI slider, OSC, ...)
/// to a real parameter value.
#[derive(Debug, Clone, Copy)]
pub enum FloatCurve {
    /// Linear interpolation across [min, max].
    Linear { min: f32, max: f32 },
    /// Logarithmic interpolation across [min, max]. `min` is floored at 1e-3.
    Log { min: f32, max: f32 },
}

impl FloatCurve {
    /// Apply the curve to a 0..127 control-surface value, returning the
    /// parameter value as `f64` (the type used throughout `InputParameters`).
    pub fn apply(&self, cc_value: u8) -> f64 {
        let t = cc_value.min(127) as f32 / 127.0;
        match *self {
            FloatCurve::Linear { min, max } => (min + t * (max - min)) as f64,
            FloatCurve::Log { min, max } => {
                let m = min.max(1e-3);
                let log_min = m.ln();
                let log_max = max.max(m).ln();
                (log_min + t * (log_max - log_min)).exp() as f64
            }
        }
    }

    /// The (min, max) extents of this curve, regardless of whether it's
    /// linear or log. Useful for sizing UI sliders.
    pub fn range(&self) -> (f32, f32) {
        match *self {
            FloatCurve::Linear { min, max } | FloatCurve::Log { min, max } => (min, max),
        }
    }
}

/// Contract a DSP effect implements to expose its float parameters to the
/// mapping layer (MIDI, future OSC, automation, ...).
///
/// Implemented on a small "kind" enum (one variant per parameter, no values).
/// `mapping.rs` iterates `ALL` to auto-generate default bindings; converting
/// `Self` into the per-effect `Param` value is done via `build`.
pub trait FloatParams: Copy + 'static + Sized {
    /// The effect's value-carrying param type (e.g. `ReverbParam`).
    type Param;

    /// The effect's "params" struct holding all current values
    /// (e.g. `ReverbParams`). Must implement `Default` so `default_value`
    /// can use it.
    type State: Default;

    /// All parameters of this effect, in display order.
    fn all() -> &'static [Self];

    /// Construct a value-carrying parameter from this kind plus a value.
    fn build(self, value: f64) -> Self::Param;

    /// Default curve to use when auto-mapping a control surface to this param.
    fn default_curve(self) -> FloatCurve;

    /// Human-readable label for UI rendering. Include units if relevant
    /// (e.g. "Pre-delay (ms)", "HPF (Hz)").
    fn name(self) -> &'static str;

    /// Read this parameter's current value out of an effect's state struct.
    fn read(self, state: &Self::State) -> f64;

    /// Default value (read from `Self::State::default()`).
    fn default_value(self) -> f64 {
        self.read(&Self::State::default())
    }
}
