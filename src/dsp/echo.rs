use crate::dsp::dsp_toolbox::constants::{PI, TWO_PI};
use crate::dsp::dsp_toolbox::math::{interpol_rt, tan};
use crate::dsp::param::{FloatCurve, FloatParams};

const ECHO_BUFFER_SIZE: usize = 131_072;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EchoParams {
    pub send: f32,
    pub fb_local: f32,
    pub fb_cross: f32,
    pub time_l_ms: f32,
    pub time_r_ms: f32,
    pub lpf_hz: f32,
}

impl Default for EchoParams {
    fn default() -> Self {
        Self {
            send: 1.0,
            fb_local: 0.4,
            fb_cross: 0.0,
            time_l_ms: 400.0,
            time_r_ms: 300.0,
            lpf_hz: 8000.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum EchoParam {
    Send(f64),
    FbLocal(f64),
    FbCross(f64),
    TimeLMs(f64),
    TimeRMs(f64),
    LpfHz(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoParamKind {
    Send,
    FbLocal,
    FbCross,
    TimeLMs,
    TimeRMs,
    LpfHz,
}

impl FloatParams for EchoParamKind {
    type Param = EchoParam;
    type State = EchoParams;

    fn all() -> &'static [Self] {
        &[
            Self::Send,
            Self::FbLocal,
            Self::FbCross,
            Self::TimeLMs,
            Self::TimeRMs,
            Self::LpfHz,
        ]
    }

    fn build(self, value: f64) -> EchoParam {
        match self {
            Self::Send => EchoParam::Send(value),
            Self::FbLocal => EchoParam::FbLocal(value),
            Self::FbCross => EchoParam::FbCross(value),
            Self::TimeLMs => EchoParam::TimeLMs(value),
            Self::TimeRMs => EchoParam::TimeRMs(value),
            Self::LpfHz => EchoParam::LpfHz(value),
        }
    }

    fn default_curve(self) -> FloatCurve {
        match self {
            Self::Send | Self::FbLocal | Self::FbCross => {
                FloatCurve::Linear { min: 0.0, max: 1.0 }
            }
            Self::TimeLMs | Self::TimeRMs => {
                FloatCurve::Linear { min: 0.0, max: 2_000.0 }
            }
            Self::LpfHz => FloatCurve::Log { min: 200.0, max: 16_000.0 },
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Send => "Send",
            Self::FbLocal => "FB Local",
            Self::FbCross => "FB Cross",
            Self::TimeLMs => "Time L (ms)",
            Self::TimeRMs => "Time R (ms)",
            Self::LpfHz => "LPF (Hz)",
        }
    }

    fn read(self, p: &EchoParams) -> f64 {
        match self {
            Self::Send => p.send as f64,
            Self::FbLocal => p.fb_local as f64,
            Self::FbCross => p.fb_cross as f64,
            Self::TimeLMs => p.time_l_ms as f64,
            Self::TimeRMs => p.time_r_ms as f64,
            Self::LpfHz => p.lpf_hz as f64,
        }
    }

    fn osc_namespace() -> &'static str {
        "/echo"
    }

    fn osc_segment(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::FbLocal => "fb_local",
            Self::FbCross => "fb_cross",
            Self::TimeLMs => "time_l_ms",
            Self::TimeRMs => "time_r_ms",
            Self::LpfHz => "lpf_hz",
        }
    }
}

pub struct Echo {
    pub out_l: f32,
    pub out_r: f32,

    params: EchoParams,
    sample_rate: f32,

    warp_const_pi: f32,
    freq_clip_min: f32,
    freq_clip_max: f32,

    time_l_samples: f32,
    time_r_samples: f32,

    hp_b0: f32,
    hp_b1: f32,
    hp_a1: f32,
    hp_state_l1: f32,
    hp_state_l2: f32,
    hp_state_r1: f32,
    hp_state_r2: f32,

    lp_b0: f32,
    lp_b1: f32,
    lp_a1: f32,
    lp_state_l1: f32,
    lp_state_l2: f32,
    lp_state_r1: f32,
    lp_state_r2: f32,

    lp2hz_b0: f32,
    lp2hz_state_l: f32,
    lp2hz_state_r: f32,

    fb_state_l: f32,
    fb_state_r: f32,

    buffer_indx: i32,
    buffer_sz_m1: i32,
    buffer_l: Vec<f32>,
    buffer_r: Vec<f32>,
}

impl Echo {
    pub fn new(sample_rate: f32, upsample_factor: u32) -> Self {
        let buffer_size = ECHO_BUFFER_SIZE * upsample_factor as usize;
        let buffer_sz_m1 = buffer_size as i32 - 1;
        let warp_const_pi = PI / sample_rate;
        let freq_clip_min = sample_rate / 24576.0;
        let freq_clip_max = sample_rate / 2.125;

        let hp_omega = tan(50.0 * warp_const_pi);
        let hp_denom = 1.0 / (1.0 + hp_omega);
        let hp_a1 = (1.0 - hp_omega) * hp_denom;
        let hp_b0 = hp_denom;
        let hp_b1 = -hp_denom;

        let lp2hz_b0 = (2.0 * TWO_PI / sample_rate).min(1.9);

        let mut echo = Self {
            out_l: 0.0,
            out_r: 0.0,
            params: EchoParams::default(),
            sample_rate,
            warp_const_pi,
            freq_clip_min,
            freq_clip_max,
            time_l_samples: 0.0,
            time_r_samples: 0.0,
            hp_b0,
            hp_b1,
            hp_a1,
            hp_state_l1: 0.0,
            hp_state_l2: 0.0,
            hp_state_r1: 0.0,
            hp_state_r2: 0.0,
            lp_b0: 0.0,
            lp_b1: 0.0,
            lp_a1: 0.0,
            lp_state_l1: 0.0,
            lp_state_l2: 0.0,
            lp_state_r1: 0.0,
            lp_state_r2: 0.0,
            lp2hz_b0,
            lp2hz_state_l: 0.0,
            lp2hz_state_r: 0.0,
            fb_state_l: 0.0,
            fb_state_r: 0.0,
            buffer_indx: 0,
            buffer_sz_m1,
            buffer_l: vec![0.0; buffer_size],
            buffer_r: vec![0.0; buffer_size],
        };
        echo.refresh();
        echo
    }

    pub fn params(&self) -> &EchoParams {
        &self.params
    }

    pub fn set_params(&mut self, params: EchoParams) {
        self.params = params;
        self.refresh();
    }

    pub fn update_param(&mut self, param: EchoParam) {
        match param {
            EchoParam::Send(v) => self.params.send = v as f32,
            EchoParam::FbLocal(v) => self.params.fb_local = v as f32,
            EchoParam::FbCross(v) => self.params.fb_cross = v as f32,
            EchoParam::TimeLMs(v) => self.params.time_l_ms = v as f32,
            EchoParam::TimeRMs(v) => self.params.time_r_ms = v as f32,
            EchoParam::LpfHz(v) => self.params.lpf_hz = v as f32,
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        let omega = self.params.lpf_hz.clamp(self.freq_clip_min, self.freq_clip_max);
        let omega = tan(omega * self.warp_const_pi);
        let denom = 1.0 / (1.0 + omega);
        self.lp_a1 = (1.0 - omega) * denom;
        self.lp_b0 = omega * denom;
        self.lp_b1 = self.lp_b0;

        let max_samples = self.buffer_sz_m1 as f32;
        self.time_l_samples = (self.params.time_l_ms * 0.001 * self.sample_rate)
            .clamp(0.0, max_samples);
        self.time_r_samples = (self.params.time_r_ms * 0.001 * self.sample_rate)
            .clamp(0.0, max_samples);
    }

    pub fn reset(&mut self) {
        self.out_l = 0.0;
        self.out_r = 0.0;
        self.hp_state_l1 = 0.0;
        self.hp_state_l2 = 0.0;
        self.hp_state_r1 = 0.0;
        self.hp_state_r2 = 0.0;
        self.lp_state_l1 = 0.0;
        self.lp_state_l2 = 0.0;
        self.lp_state_r1 = 0.0;
        self.lp_state_r2 = 0.0;
        self.lp2hz_state_l = 0.0;
        self.lp2hz_state_r = 0.0;
        self.fb_state_l = 0.0;
        self.fb_state_r = 0.0;
        self.buffer_l.fill(0.0);
        self.buffer_r.fill(0.0);
    }

    pub fn apply(&mut self, raw_l: f32, raw_r: f32) {
        let bi = self.buffer_indx;
        let mask = self.buffer_sz_m1;

        // ---------- Left ----------
        let mut t = raw_l * self.params.send
            + self.fb_state_l * self.params.fb_local
            + self.fb_state_r * self.params.fb_cross;
        self.buffer_l[bi as usize] = t;

        // 2 Hz LP smoother on left delay time
        t = self.time_l_samples - self.lp2hz_state_l;
        t = t * self.lp2hz_b0 + self.lp2hz_state_l;
        self.lp2hz_state_l = t;

        let i0_i = (t - 0.5).round() as i32;
        let frac = t - i0_i as f32;
        let im1_i = i32::max(i0_i - 1, 0);
        let ip1_i = i0_i + 1;
        let ip2_i = i0_i + 2;
        let im1 = ((bi - im1_i) & mask) as usize;
        let i0 = ((bi - i0_i) & mask) as usize;
        let ip1 = ((bi - ip1_i) & mask) as usize;
        let ip2 = ((bi - ip2_i) & mask) as usize;
        let tap_l = interpol_rt(
            frac,
            self.buffer_l[im1],
            self.buffer_l[i0],
            self.buffer_l[ip1],
            self.buffer_l[ip2],
        );

        // 1-pole LP on tap (output path)
        let lp_out_l = self.lp_b0 * tap_l
            + self.lp_b1 * self.lp_state_l1
            + self.lp_a1 * self.lp_state_l2;
        self.lp_state_l1 = tap_l;
        self.lp_state_l2 = lp_out_l;

        // 1-pole HP at 50 Hz (feedback path only, for DC removal in the loop)
        self.fb_state_l = self.hp_b0 * lp_out_l
            + self.hp_b1 * self.hp_state_l1
            + self.hp_a1 * self.hp_state_l2;
        self.hp_state_l1 = lp_out_l;
        self.hp_state_l2 = self.fb_state_l;

        self.out_l = lp_out_l;

        // ---------- Right ----------
        let mut t = raw_r * self.params.send
            + self.fb_state_r * self.params.fb_local
            + self.fb_state_l * self.params.fb_cross;
        self.buffer_r[bi as usize] = t;

        t = self.time_r_samples - self.lp2hz_state_r;
        t = t * self.lp2hz_b0 + self.lp2hz_state_r;
        self.lp2hz_state_r = t;

        let i0_i = (t - 0.5).round() as i32;
        let frac = t - i0_i as f32;
        let im1_i = i32::max(i0_i - 1, 0);
        let ip1_i = i0_i + 1;
        let ip2_i = i0_i + 2;
        let im1 = ((bi - im1_i) & mask) as usize;
        let i0 = ((bi - i0_i) & mask) as usize;
        let ip1 = ((bi - ip1_i) & mask) as usize;
        let ip2 = ((bi - ip2_i) & mask) as usize;
        let tap_r = interpol_rt(
            frac,
            self.buffer_r[im1],
            self.buffer_r[i0],
            self.buffer_r[ip1],
            self.buffer_r[ip2],
        );

        let lp_out_r = self.lp_b0 * tap_r
            + self.lp_b1 * self.lp_state_r1
            + self.lp_a1 * self.lp_state_r2;
        self.lp_state_r1 = tap_r;
        self.lp_state_r2 = lp_out_r;

        self.fb_state_r = self.hp_b0 * lp_out_r
            + self.hp_b1 * self.hp_state_r1
            + self.hp_a1 * self.hp_state_r2;
        self.hp_state_r1 = lp_out_r;
        self.hp_state_r2 = self.fb_state_r;

        self.out_r = lp_out_r;

        self.buffer_indx = (bi + 1) & mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_feedback_params(time_ms: f32) -> EchoParams {
        EchoParams {
            send: 1.0,
            fb_local: 0.0,
            fb_cross: 0.0,
            time_l_ms: time_ms,
            time_r_ms: time_ms,
            lpf_hz: 16_000.0,
        }
    }

    fn warm_up(echo: &mut Echo, samples: usize) {
        for _ in 0..samples {
            echo.apply(0.0, 0.0);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut echo = Echo::new(48_000.0, 1);
        echo.set_params(EchoParams::default());
        for _ in 0..96_000 {
            echo.apply(0.0, 0.0);
            assert!(echo.out_l.is_finite() && echo.out_r.is_finite());
            assert!(echo.out_l.abs() < 1e-6);
            assert!(echo.out_r.abs() < 1e-6);
        }
    }

    #[test]
    fn impulse_creates_delayed_tap() {
        let time_ms = 100.0;
        let mut echo = Echo::new(48_000.0, 1);
        echo.set_params(no_feedback_params(time_ms));
        warm_up(&mut echo, 96_000);

        echo.apply(1.0, 0.0);

        let expected_delay = (time_ms * 0.001 * 48_000.0) as i32;
        let mut peak = 0.0f32;
        let mut peak_at: i32 = 0;
        let scan_len = expected_delay + 2_000;
        for i in 0..scan_len {
            echo.apply(0.0, 0.0);
            if echo.out_l.abs() > peak {
                peak = echo.out_l.abs();
                peak_at = i;
            }
        }

        assert!(peak > 0.001, "no echo detected, peak={peak}");
        let delta = (peak_at - expected_delay).abs();
        assert!(
            delta < 200,
            "echo arrived at sample {peak_at}, expected ~{expected_delay} (delta={delta})"
        );
    }

    #[test]
    fn feedback_decays_under_unity_gain() {
        let mut echo = Echo::new(48_000.0, 1);
        echo.set_params(EchoParams {
            send: 1.0,
            fb_local: 0.5,
            fb_cross: 0.0,
            time_l_ms: 100.0,
            time_r_ms: 100.0,
            lpf_hz: 8_000.0,
        });
        warm_up(&mut echo, 96_000);

        echo.apply(1.0, 1.0);
        let mut peak = 0.0f32;
        for _ in 0..480_000 {
            echo.apply(0.0, 0.0);
            assert!(echo.out_l.is_finite() && echo.out_r.is_finite());
            peak = peak.max(echo.out_l.abs()).max(echo.out_r.abs());
        }
        assert!(peak < 5.0, "echo amplified beyond bound: {peak}");
    }

    #[test]
    fn no_send_no_output() {
        let mut echo = Echo::new(48_000.0, 1);
        echo.set_params(EchoParams {
            send: 0.0,
            ..EchoParams::default()
        });
        warm_up(&mut echo, 4_800);

        for _ in 0..48_000 {
            echo.apply(1.0, 1.0);
        }
        assert!(
            echo.out_l.abs() < 1e-6 && echo.out_r.abs() < 1e-6,
            "send=0 should silence echo, got ({}, {})",
            echo.out_l,
            echo.out_r
        );
    }
}
