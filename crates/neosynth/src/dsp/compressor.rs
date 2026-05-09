use crate::dsp::param::{FloatCurve, FloatParams};
use serde::{Deserialize, Serialize};

/// Persisted compressor parameter state. Mirrors the live `Compressor` so
/// app state can be saved + replayed at startup.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        // Default to "effectively off": threshold above any signal we'll see.
        // The user moves the threshold down until the GR meter starts moving.
        Self {
            threshold_db: 0.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 100.0,
            knee_db: 6.0,
            makeup_db: 0.0,
        }
    }
}

/// Value-carrying parameter event sent over the realtime channel.
#[derive(Debug, Clone, Copy)]
pub enum CompressorParam {
    ThresholdDb(f64),
    Ratio(f64),
    AttackMs(f64),
    ReleaseMs(f64),
    KneeDb(f64),
    MakeupDb(f64),
}

/// Kind-only enum used to enumerate parameters for MIDI / OSC auto-mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressorParamKind {
    ThresholdDb,
    Ratio,
    AttackMs,
    ReleaseMs,
    KneeDb,
    MakeupDb,
}

impl FloatParams for CompressorParamKind {
    type Param = CompressorParam;
    type State = CompressorParams;

    fn all() -> &'static [Self] {
        &[
            Self::ThresholdDb,
            Self::Ratio,
            Self::AttackMs,
            Self::ReleaseMs,
            Self::KneeDb,
            Self::MakeupDb,
        ]
    }

    fn build(self, value: f64) -> CompressorParam {
        match self {
            Self::ThresholdDb => CompressorParam::ThresholdDb(value),
            Self::Ratio => CompressorParam::Ratio(value),
            Self::AttackMs => CompressorParam::AttackMs(value),
            Self::ReleaseMs => CompressorParam::ReleaseMs(value),
            Self::KneeDb => CompressorParam::KneeDb(value),
            Self::MakeupDb => CompressorParam::MakeupDb(value),
        }
    }

    fn default_curve(self) -> FloatCurve {
        match self {
            Self::ThresholdDb => FloatCurve::Linear { min: -60.0, max: 0.0 },
            Self::Ratio => FloatCurve::Linear { min: 1.0, max: 20.0 },
            Self::AttackMs => FloatCurve::Log { min: 0.1, max: 100.0 },
            Self::ReleaseMs => FloatCurve::Log { min: 10.0, max: 2000.0 },
            Self::KneeDb => FloatCurve::Linear { min: 0.0, max: 24.0 },
            Self::MakeupDb => FloatCurve::Linear { min: -12.0, max: 24.0 },
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::ThresholdDb => "Threshold (dB)",
            Self::Ratio => "Ratio",
            Self::AttackMs => "Attack (ms)",
            Self::ReleaseMs => "Release (ms)",
            Self::KneeDb => "Knee (dB)",
            Self::MakeupDb => "Makeup (dB)",
        }
    }

    fn read(self, state: &Self::State) -> f64 {
        match self {
            Self::ThresholdDb => state.threshold_db as f64,
            Self::Ratio => state.ratio as f64,
            Self::AttackMs => state.attack_ms as f64,
            Self::ReleaseMs => state.release_ms as f64,
            Self::KneeDb => state.knee_db as f64,
            Self::MakeupDb => state.makeup_db as f64,
        }
    }

    fn osc_namespace() -> &'static str {
        "/compressor"
    }

    fn osc_segment(self) -> &'static str {
        match self {
            Self::ThresholdDb => "threshold_db",
            Self::Ratio => "ratio",
            Self::AttackMs => "attack_ms",
            Self::ReleaseMs => "release_ms",
            Self::KneeDb => "knee_db",
            Self::MakeupDb => "makeup_db",
        }
    }
}

/// Stereo-linked feed-forward compressor with peak detection, separate
/// attack/release smoothing in the log domain, and a soft knee.
///
/// Side-chain is the louder of |L|, |R|, so both channels receive the same
/// gain — this preserves stereo image (no level-dependent panning).
pub struct Compressor {
    params: CompressorParams,
    sample_rate: f32,

    /// Detector state in dB (smoothed input level).
    detector_db: f32,
    /// Cached per-sample smoothing coefficients derived from attack / release.
    alpha_attack: f32,
    alpha_release: f32,

    /// Latest per-sample gain reduction in dB (always ≥ 0). Reading this
    /// inside the per-period loop and tracking the max gives a useful
    /// peak-GR meter.
    pub gr_db: f32,

    /// Outputs after compression + makeup.
    pub out_l: f32,
    pub out_r: f32,
}

const FLOOR_DB: f32 = -120.0;

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        let mut c = Self {
            params: CompressorParams::default(),
            sample_rate,
            detector_db: FLOOR_DB,
            alpha_attack: 0.0,
            alpha_release: 0.0,
            gr_db: 0.0,
            out_l: 0.0,
            out_r: 0.0,
        };
        c.refresh();
        c
    }

    pub fn update_param(&mut self, p: CompressorParam) {
        match p {
            CompressorParam::ThresholdDb(v) => self.params.threshold_db = v as f32,
            CompressorParam::Ratio(v) => self.params.ratio = (v as f32).max(1.0),
            CompressorParam::AttackMs(v) => self.params.attack_ms = (v as f32).max(0.01),
            CompressorParam::ReleaseMs(v) => self.params.release_ms = (v as f32).max(0.1),
            CompressorParam::KneeDb(v) => self.params.knee_db = (v as f32).max(0.0),
            CompressorParam::MakeupDb(v) => self.params.makeup_db = v as f32,
        }
        self.refresh();
    }

    #[cfg(test)]
    pub fn set_params(&mut self, params: CompressorParams) {
        self.params = params;
        self.refresh();
    }

    fn refresh(&mut self) {
        // alpha = exp(-1 / (tau_seconds * sample_rate))
        // After τ samples the smoother has reached ≈ 1 - 1/e of the target.
        let to_alpha = |ms: f32| -> f32 {
            let tau_samples = (ms.max(0.01) * 0.001) * self.sample_rate;
            (-1.0 / tau_samples).exp()
        };
        self.alpha_attack = to_alpha(self.params.attack_ms);
        self.alpha_release = to_alpha(self.params.release_ms);
    }

    #[inline]
    pub fn apply(&mut self, l: f32, r: f32) {
        // Stereo-linked peak detection.
        let abs_max = l.abs().max(r.abs()).max(1e-9);
        let level_db = 20.0 * abs_max.log10();

        // One-pole smoother with separate attack / release coefficients.
        let alpha = if level_db > self.detector_db {
            self.alpha_attack
        } else {
            self.alpha_release
        };
        self.detector_db = level_db + alpha * (self.detector_db - level_db);

        // Soft-knee gain computer. `over` = how far the smoothed level is
        // above the threshold. Outside the knee zone it's a hard ratio bend;
        // inside, a smooth quadratic interpolation.
        let over = self.detector_db - self.params.threshold_db;
        let half_knee = self.params.knee_db * 0.5;
        let slope = 1.0 - 1.0 / self.params.ratio.max(1.0);
        let gr_db = if over <= -half_knee {
            0.0
        } else if over >= half_knee {
            over * slope
        } else {
            let x = over + half_knee;
            slope * x * x / (2.0 * self.params.knee_db.max(1e-6))
        };

        self.gr_db = gr_db.max(0.0);
        let total_gain_db = -self.gr_db + self.params.makeup_db;
        let gain = 10f32.powf(total_gain_db * 0.05);

        self.out_l = l * gain;
        self.out_r = r * gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_compression_below_threshold() {
        let mut c = Compressor::new(48_000.0);
        c.set_params(CompressorParams {
            threshold_db: -6.0,
            ratio: 4.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        });
        // Settle on -20 dB sine — well below threshold. After many samples
        // the detector should be ≈ -20 dB and gr_db should be 0.
        let amp = 0.1; // ~ -20 dB
        for i in 0..2_000 {
            let s = amp * (i as f32 * 0.1).sin();
            c.apply(s, s);
        }
        assert!(c.gr_db < 0.5, "gr_db = {} (expected ~0)", c.gr_db);
    }

    #[test]
    fn compresses_above_threshold() {
        let mut c = Compressor::new(48_000.0);
        c.set_params(CompressorParams {
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        });
        // 0 dB sine = 20 dB above threshold → expect (1 - 1/4) * 20 = 15 dB GR.
        for i in 0..4_000 {
            let s = (i as f32 * 0.1).sin();
            c.apply(s, s);
        }
        assert!(
            (c.gr_db - 15.0).abs() < 1.0,
            "gr_db = {} (expected ≈15)",
            c.gr_db
        );
    }

    #[test]
    fn ratio_one_is_unity_gain() {
        let mut c = Compressor::new(48_000.0);
        c.set_params(CompressorParams {
            threshold_db: -60.0,
            ratio: 1.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            knee_db: 0.0,
            makeup_db: 0.0,
        });
        for i in 0..2_000 {
            let s = (i as f32 * 0.1).sin();
            c.apply(s, s);
        }
        assert!(c.gr_db.abs() < 0.5, "ratio=1 should not reduce: {}", c.gr_db);
    }
}
