//! Roland RE-201 Space Echo flavoured tape delay.
//!
//! Three "playback heads" tap a single tape buffer at 1/3, 2/3 and 1/1 of the
//! `repeat_rate_ms` setting. Each head has its own mix level so users can
//! reproduce any of the seven RE-201 mode combinations (head 1, 2, 3, 1+2,
//! 2+3, 1+3, 1+2+3) by setting the levels — or any continuous blend in
//! between, which the original couldn't do.
//!
//! Tape character comes from:
//!   * `tanh` saturation at the write head (input drive) and in the feedback
//!     loop. This naturally limits self-oscillation so high `intensity` is
//!     musical instead of explosive.
//!   * Wow + flutter: two summed sinusoids modulating each head's read
//!     position. Wow is slow (~0.5 Hz) and deep, flutter is fast (~6 Hz) and
//!     shallow.
//!   * High-frequency rolloff on the read side: a 1-pole LP simulating tape's
//!     loss of treble.
//!
//! Stereo: two independent tape buffers, one per channel, with shared params.
//! No cross-channel feedback (would be more like a stereo delay; tape units
//! were classically mono and this preserves that feel while running in stereo
//! when fed stereo content).

use crate::dsp::param::{FloatCurve, FloatParams};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct TapeDelayParams {
    pub send: f32,
    pub repeat_rate_ms: f32,
    pub intensity: f32,
    pub h1_level: f32,
    pub h2_level: f32,
    pub h3_level: f32,
    pub saturation_drive: f32,
    pub hf_rolloff_hz: f32,
    pub wow_depth: f32,
    pub wow_rate_hz: f32,
    pub flutter_depth: f32,
    pub flutter_rate_hz: f32,
}

impl Default for TapeDelayParams {
    fn default() -> Self {
        Self {
            send: 0.0,
            repeat_rate_ms: 350.0,
            intensity: 0.4,
            h1_level: 0.0,
            h2_level: 0.0,
            h3_level: 1.0, // mode 3 (longest head only)
            saturation_drive: 0.3,
            hf_rolloff_hz: 6000.0,
            wow_depth: 0.3,
            wow_rate_hz: 0.5,
            flutter_depth: 0.2,
            flutter_rate_hz: 6.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TapeDelayParam {
    Send(f64),
    RepeatRateMs(f64),
    Intensity(f64),
    H1Level(f64),
    H2Level(f64),
    H3Level(f64),
    SaturationDrive(f64),
    HfRolloffHz(f64),
    WowDepth(f64),
    WowRateHz(f64),
    FlutterDepth(f64),
    FlutterRateHz(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapeDelayParamKind {
    Send,
    RepeatRateMs,
    Intensity,
    H1Level,
    H2Level,
    H3Level,
    SaturationDrive,
    HfRolloffHz,
    WowDepth,
    WowRateHz,
    FlutterDepth,
    FlutterRateHz,
}

impl FloatParams for TapeDelayParamKind {
    type Param = TapeDelayParam;
    type State = TapeDelayParams;

    fn all() -> &'static [Self] {
        &[
            Self::Send,
            Self::RepeatRateMs,
            Self::Intensity,
            Self::H1Level,
            Self::H2Level,
            Self::H3Level,
            Self::SaturationDrive,
            Self::HfRolloffHz,
            Self::WowDepth,
            Self::WowRateHz,
            Self::FlutterDepth,
            Self::FlutterRateHz,
        ]
    }

    fn build(self, value: f64) -> TapeDelayParam {
        match self {
            Self::Send => TapeDelayParam::Send(value),
            Self::RepeatRateMs => TapeDelayParam::RepeatRateMs(value),
            Self::Intensity => TapeDelayParam::Intensity(value),
            Self::H1Level => TapeDelayParam::H1Level(value),
            Self::H2Level => TapeDelayParam::H2Level(value),
            Self::H3Level => TapeDelayParam::H3Level(value),
            Self::SaturationDrive => TapeDelayParam::SaturationDrive(value),
            Self::HfRolloffHz => TapeDelayParam::HfRolloffHz(value),
            Self::WowDepth => TapeDelayParam::WowDepth(value),
            Self::WowRateHz => TapeDelayParam::WowRateHz(value),
            Self::FlutterDepth => TapeDelayParam::FlutterDepth(value),
            Self::FlutterRateHz => TapeDelayParam::FlutterRateHz(value),
        }
    }

    fn default_curve(self) -> FloatCurve {
        match self {
            Self::Send => FloatCurve::Linear { min: 0.0, max: 1.0 },
            Self::RepeatRateMs => FloatCurve::Log { min: 50.0, max: 2000.0 },
            Self::Intensity => FloatCurve::Linear { min: 0.0, max: 1.5 },
            Self::H1Level | Self::H2Level | Self::H3Level => {
                FloatCurve::Linear { min: 0.0, max: 1.0 }
            }
            Self::SaturationDrive => FloatCurve::Linear { min: 0.0, max: 1.0 },
            Self::HfRolloffHz => FloatCurve::Log { min: 1000.0, max: 15000.0 },
            Self::WowDepth => FloatCurve::Linear { min: 0.0, max: 1.0 },
            Self::WowRateHz => FloatCurve::Log { min: 0.1, max: 2.0 },
            Self::FlutterDepth => FloatCurve::Linear { min: 0.0, max: 1.0 },
            Self::FlutterRateHz => FloatCurve::Log { min: 2.0, max: 15.0 },
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Send => "Send",
            Self::RepeatRateMs => "Repeat Rate (ms)",
            Self::Intensity => "Intensity",
            Self::H1Level => "Head 1 Level",
            Self::H2Level => "Head 2 Level",
            Self::H3Level => "Head 3 Level",
            Self::SaturationDrive => "Tape Drive",
            Self::HfRolloffHz => "HF Rolloff (Hz)",
            Self::WowDepth => "Wow Depth",
            Self::WowRateHz => "Wow Rate (Hz)",
            Self::FlutterDepth => "Flutter Depth",
            Self::FlutterRateHz => "Flutter Rate (Hz)",
        }
    }

    fn read(self, state: &Self::State) -> f64 {
        match self {
            Self::Send => state.send as f64,
            Self::RepeatRateMs => state.repeat_rate_ms as f64,
            Self::Intensity => state.intensity as f64,
            Self::H1Level => state.h1_level as f64,
            Self::H2Level => state.h2_level as f64,
            Self::H3Level => state.h3_level as f64,
            Self::SaturationDrive => state.saturation_drive as f64,
            Self::HfRolloffHz => state.hf_rolloff_hz as f64,
            Self::WowDepth => state.wow_depth as f64,
            Self::WowRateHz => state.wow_rate_hz as f64,
            Self::FlutterDepth => state.flutter_depth as f64,
            Self::FlutterRateHz => state.flutter_rate_hz as f64,
        }
    }

    fn osc_namespace() -> &'static str {
        "/tape_delay"
    }

    fn osc_segment(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::RepeatRateMs => "repeat_rate_ms",
            Self::Intensity => "intensity",
            Self::H1Level => "h1_level",
            Self::H2Level => "h2_level",
            Self::H3Level => "h3_level",
            Self::SaturationDrive => "saturation_drive",
            Self::HfRolloffHz => "hf_rolloff_hz",
            Self::WowDepth => "wow_depth",
            Self::WowRateHz => "wow_rate_hz",
            Self::FlutterDepth => "flutter_depth",
            Self::FlutterRateHz => "flutter_rate_hz",
        }
    }
}

/// One tape track — write head, three read heads with wow/flutter, HF rolloff.
struct TapeTrack {
    buffer: Vec<f32>,
    mask: usize, // buffer.len() must be a power of two
    write_idx: usize,
    /// 1-pole LP state for the tape head HF rolloff.
    lp_state: f32,
    /// Saturated feedback signal carried over from the previous sample.
    fb: f32,
}

impl TapeTrack {
    fn new(max_samples: usize) -> Self {
        let size = max_samples.next_power_of_two().max(1024);
        Self {
            buffer: vec![0.0; size],
            mask: size - 1,
            write_idx: 0,
            lp_state: 0.0,
            fb: 0.0,
        }
    }

    /// Linear interpolation read at fractional sample distance `tap_samples`
    /// behind the current write index.
    #[inline]
    fn read_tap(&self, tap_samples: f32) -> f32 {
        // Clamp so we don't underflow the (unsigned) buffer math.
        let max_back = (self.buffer.len() - 2) as f32;
        let t = tap_samples.clamp(1.0, max_back);
        let int_part = t.floor() as usize;
        let frac = t - int_part as f32;
        let i0 = (self.write_idx + self.buffer.len() - int_part) & self.mask;
        let i1 = (i0 + self.buffer.len() - 1) & self.mask;
        let s0 = self.buffer[i0];
        let s1 = self.buffer[i1];
        s0 * (1.0 - frac) + s1 * frac
    }

    #[inline]
    fn write(&mut self, s: f32) {
        self.buffer[self.write_idx] = s;
        self.write_idx = (self.write_idx + 1) & self.mask;
    }
}

pub struct TapeDelay {
    sample_rate: f32,
    params: TapeDelayParams,

    track_l: TapeTrack,
    track_r: TapeTrack,

    // LFOs — phase accumulators, advanced once per sample.
    wow_phase: f32,
    flutter_phase: f32,

    // Cached per-block (recomputed in update_param).
    /// 1-pole LP coefficient for the HF rolloff. Output = lp_a * input + (1 - lp_a) * state.
    lp_a: f32,
    /// Wow / flutter modulation depths in samples (rather than ms).
    wow_depth_samples: f32,
    flutter_depth_samples: f32,
    /// LFO phase increments per sample.
    wow_inc: f32,
    flutter_inc: f32,

    pub out_l: f32,
    pub out_r: f32,
}

impl TapeDelay {
    pub fn new(sample_rate: f32) -> Self {
        // Max delay needs to fit longest head at slowest tape speed plus
        // wow/flutter excursion. 2000 ms × 1.0 + ~10 % headroom = ~2.2 s.
        let max_samples = (sample_rate * 2.5) as usize;
        let mut td = Self {
            sample_rate,
            params: TapeDelayParams::default(),
            track_l: TapeTrack::new(max_samples),
            track_r: TapeTrack::new(max_samples),
            wow_phase: 0.0,
            flutter_phase: 0.0,
            lp_a: 0.0,
            wow_depth_samples: 0.0,
            flutter_depth_samples: 0.0,
            wow_inc: 0.0,
            flutter_inc: 0.0,
            out_l: 0.0,
            out_r: 0.0,
        };
        td.refresh();
        td
    }

    pub fn update_param(&mut self, p: TapeDelayParam) {
        match p {
            TapeDelayParam::Send(v) => self.params.send = (v as f32).clamp(0.0, 1.0),
            TapeDelayParam::RepeatRateMs(v) => {
                self.params.repeat_rate_ms = (v as f32).clamp(10.0, 2000.0)
            }
            TapeDelayParam::Intensity(v) => self.params.intensity = (v as f32).clamp(0.0, 2.0),
            TapeDelayParam::H1Level(v) => self.params.h1_level = (v as f32).clamp(0.0, 1.0),
            TapeDelayParam::H2Level(v) => self.params.h2_level = (v as f32).clamp(0.0, 1.0),
            TapeDelayParam::H3Level(v) => self.params.h3_level = (v as f32).clamp(0.0, 1.0),
            TapeDelayParam::SaturationDrive(v) => {
                self.params.saturation_drive = (v as f32).clamp(0.0, 1.0)
            }
            TapeDelayParam::HfRolloffHz(v) => {
                self.params.hf_rolloff_hz = (v as f32).clamp(500.0, 20000.0)
            }
            TapeDelayParam::WowDepth(v) => self.params.wow_depth = (v as f32).clamp(0.0, 1.0),
            TapeDelayParam::WowRateHz(v) => self.params.wow_rate_hz = (v as f32).clamp(0.05, 5.0),
            TapeDelayParam::FlutterDepth(v) => {
                self.params.flutter_depth = (v as f32).clamp(0.0, 1.0)
            }
            TapeDelayParam::FlutterRateHz(v) => {
                self.params.flutter_rate_hz = (v as f32).clamp(1.0, 30.0)
            }
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        // 1-pole LP for HF rolloff — RC equivalent. lp_a = dt / (RC + dt).
        let omega = TAU * self.params.hf_rolloff_hz / self.sample_rate;
        let alpha = omega / (omega + 1.0);
        self.lp_a = alpha.clamp(0.001, 0.99);

        // Wow excursion ≈ ±5 % of repeat rate at full depth. Flutter is
        // smaller (~1 %) but faster; both scale with the chosen tape speed
        // so they sound consistent across delay-time settings.
        let base_samples = self.params.repeat_rate_ms * 0.001 * self.sample_rate;
        self.wow_depth_samples = self.params.wow_depth * base_samples * 0.05;
        self.flutter_depth_samples = self.params.flutter_depth * base_samples * 0.01;

        self.wow_inc = TAU * self.params.wow_rate_hz / self.sample_rate;
        self.flutter_inc = TAU * self.params.flutter_rate_hz / self.sample_rate;
    }

    #[inline]
    pub fn apply(&mut self, in_l: f32, in_r: f32) {
        // Sum of LFOs in samples — same modulation source for both tracks
        // so the stereo image stays coherent.
        let modulation =
            self.wow_depth_samples * self.wow_phase.sin()
                + self.flutter_depth_samples * self.flutter_phase.sin();
        self.wow_phase += self.wow_inc;
        if self.wow_phase > TAU {
            self.wow_phase -= TAU;
        }
        self.flutter_phase += self.flutter_inc;
        if self.flutter_phase > TAU {
            self.flutter_phase -= TAU;
        }

        let base_samples = self.params.repeat_rate_ms * 0.001 * self.sample_rate;
        let tap1 = base_samples * (1.0 / 3.0) + modulation;
        let tap2 = base_samples * (2.0 / 3.0) + modulation;
        let tap3 = base_samples + modulation;

        // Drive the input through the saturation curve. Drive = 0 → unity,
        // drive = 1 → ~6 dB pre-gain into tanh which adds substantial harmonic
        // content.
        let drive = 1.0 + self.params.saturation_drive * 1.5;

        let send = self.params.send;

        // ----- Left track -----
        let l_in = (in_l * send + self.track_l.fb).tanh();
        // We saturate post-sum to model "tape input bias" where loud signals
        // compress regardless of source.
        let l_pre = (l_in * drive).tanh() / drive.max(1.0);
        self.track_l.write(l_pre);

        let l_h1 = self.track_l.read_tap(tap1);
        let l_h2 = self.track_l.read_tap(tap2);
        let l_h3 = self.track_l.read_tap(tap3);
        let l_sum = l_h1 * self.params.h1_level
            + l_h2 * self.params.h2_level
            + l_h3 * self.params.h3_level;

        // HF rolloff (1-pole LP).
        self.track_l.lp_state = self.lp_a * l_sum + (1.0 - self.lp_a) * self.track_l.lp_state;
        let l_out = self.track_l.lp_state;

        // Feedback through saturation — bounds self-oscillation to a stable level.
        self.track_l.fb = (l_out * self.params.intensity).tanh() * 0.95;

        // ----- Right track (same shape) -----
        let r_in = (in_r * send + self.track_r.fb).tanh();
        let r_pre = (r_in * drive).tanh() / drive.max(1.0);
        self.track_r.write(r_pre);

        let r_h1 = self.track_r.read_tap(tap1);
        let r_h2 = self.track_r.read_tap(tap2);
        let r_h3 = self.track_r.read_tap(tap3);
        let r_sum = r_h1 * self.params.h1_level
            + r_h2 * self.params.h2_level
            + r_h3 * self.params.h3_level;

        self.track_r.lp_state = self.lp_a * r_sum + (1.0 - self.lp_a) * self.track_r.lp_state;
        let r_out = self.track_r.lp_state;

        self.track_r.fb = (r_out * self.params.intensity).tanh() * 0.95;

        self.out_l = l_out;
        self.out_r = r_out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_in_silence_out_after_settling() {
        let mut td = TapeDelay::new(48_000.0);
        for _ in 0..96_000 {
            td.apply(0.0, 0.0);
        }
        assert!(td.out_l.abs() < 1e-6);
        assert!(td.out_r.abs() < 1e-6);
    }

    #[test]
    fn impulse_creates_delayed_taps() {
        let mut td = TapeDelay::new(48_000.0);
        // Single head 3 only, no feedback, no modulation, no saturation,
        // long HF rolloff so the impulse survives.
        td.update_param(TapeDelayParam::Send(1.0));
        td.update_param(TapeDelayParam::Intensity(0.0));
        td.update_param(TapeDelayParam::H1Level(0.0));
        td.update_param(TapeDelayParam::H2Level(0.0));
        td.update_param(TapeDelayParam::H3Level(1.0));
        td.update_param(TapeDelayParam::SaturationDrive(0.0));
        td.update_param(TapeDelayParam::WowDepth(0.0));
        td.update_param(TapeDelayParam::FlutterDepth(0.0));
        td.update_param(TapeDelayParam::HfRolloffHz(15_000.0));
        td.update_param(TapeDelayParam::RepeatRateMs(100.0));

        // Fire impulse, then run silence for ~120 ms looking for non-trivial
        // energy near the 100 ms mark.
        td.apply(1.0, 1.0);
        let mut peak_pos = 0;
        let mut peak_val = 0.0_f32;
        for i in 0..6_000 {
            td.apply(0.0, 0.0);
            if td.out_l.abs() > peak_val {
                peak_val = td.out_l.abs();
                peak_pos = i;
            }
        }
        assert!(peak_val > 0.05, "no echo seen, peak={}", peak_val);
        // 100 ms at 48 kHz = 4800 samples. Allow ±5 % for filter delay.
        assert!(
            (peak_pos as i32 - 4800).abs() < 240,
            "peak at sample {} (expected ~4800)",
            peak_pos
        );
    }

    #[test]
    fn feedback_decays_under_unity() {
        let mut td = TapeDelay::new(48_000.0);
        td.update_param(TapeDelayParam::Send(1.0));
        td.update_param(TapeDelayParam::Intensity(0.5));
        td.update_param(TapeDelayParam::H3Level(1.0));
        td.update_param(TapeDelayParam::H1Level(0.0));
        td.update_param(TapeDelayParam::H2Level(0.0));
        td.update_param(TapeDelayParam::WowDepth(0.0));
        td.update_param(TapeDelayParam::FlutterDepth(0.0));
        td.update_param(TapeDelayParam::SaturationDrive(0.0));
        td.update_param(TapeDelayParam::RepeatRateMs(100.0));

        td.apply(1.0, 1.0);
        let mut max_after = 0.0_f32;
        for _ in 0..96_000 {
            td.apply(0.0, 0.0);
            max_after = max_after.max(td.out_l.abs());
        }
        // Feedback < 1 with saturation: stays bounded.
        assert!(max_after < 1.5, "runaway feedback: {}", max_after);
    }
}
