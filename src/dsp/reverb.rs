use crate::dsp::dsp_toolbox::constants::PI;
use crate::dsp::dsp_toolbox::math::{interpol_rt, tan};
use crate::dsp::param::{FloatCurve, FloatParams};

const REVERB_BUFFER_SIZE: usize = 16384;

const REV_G_1: f32 = 0.617748;
const REV_G_2: f32 = 0.630809;
const REV_G_3: f32 = 0.64093;
const REV_G_4: f32 = 0.653011;

const REV_DEL_L1: u32 = 280;
const REV_DEL_L2: u32 = 1122;
const REV_DEL_L3: u32 = 862;
const REV_DEL_L4: u32 = 466;
const REV_DEL_L5: u32 = 718;
const REV_DEL_L6: u32 = 1030;
const REV_DEL_L7: u32 = 886;
const REV_DEL_L8: u32 = 1216;
const REV_DEL_L9: u32 = 2916;

const REV_DEL_R1: u32 = 378;
const REV_DEL_R2: u32 = 1102;
const REV_DEL_R3: u32 = 928;
const REV_DEL_R4: u32 = 490;
const REV_DEL_R5: u32 = 682;
const REV_DEL_R6: u32 = 1018;
const REV_DEL_R7: u32 = 858;
const REV_DEL_R8: u32 = 1366;
const REV_DEL_R9: u32 = 2676;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    pub size: f32,
    pub feedback: f32,
    pub balance: f32,
    pub pre_delay_ms: f32,
    pub hpf_hz: f32,
    pub lpf_hz: f32,
    pub chorus: f32,
    pub send: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            size: 0.5,
            feedback: 0.7,
            balance: 0.5,
            pre_delay_ms: 5.0,
            hpf_hz: 200.0,
            lpf_hz: 8000.0,
            chorus: 0.5,
            send: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReverbParam {
    Size(f64),
    Feedback(f64),
    Balance(f64),
    PreDelayMs(f64),
    HpfHz(f64),
    LpfHz(f64),
    Chorus(f64),
    Send(f64),
}

/// Value-less identifier for each reverb parameter. Used by mapping layers
/// (MIDI today, OSC/automation later) to refer to a parameter without
/// constructing a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverbParamKind {
    Size,
    Feedback,
    Balance,
    PreDelayMs,
    HpfHz,
    LpfHz,
    Chorus,
    Send,
}

impl FloatParams for ReverbParamKind {
    type Param = ReverbParam;
    type State = ReverbParams;

    fn all() -> &'static [Self] {
        &[
            Self::Size,
            Self::Feedback,
            Self::Balance,
            Self::PreDelayMs,
            Self::HpfHz,
            Self::LpfHz,
            Self::Chorus,
            Self::Send,
        ]
    }

    fn build(self, value: f64) -> ReverbParam {
        match self {
            Self::Size => ReverbParam::Size(value),
            Self::Feedback => ReverbParam::Feedback(value),
            Self::Balance => ReverbParam::Balance(value),
            Self::PreDelayMs => ReverbParam::PreDelayMs(value),
            Self::HpfHz => ReverbParam::HpfHz(value),
            Self::LpfHz => ReverbParam::LpfHz(value),
            Self::Chorus => ReverbParam::Chorus(value),
            Self::Send => ReverbParam::Send(value),
        }
    }

    fn default_curve(self) -> FloatCurve {
        match self {
            Self::Size
            | Self::Feedback
            | Self::Balance
            | Self::Chorus
            | Self::Send => FloatCurve::Linear { min: 0.0, max: 1.0 },
            Self::PreDelayMs => FloatCurve::Linear { min: 0.0, max: 200.0 },
            Self::HpfHz => FloatCurve::Log { min: 20.0, max: 2_000.0 },
            Self::LpfHz => FloatCurve::Log { min: 200.0, max: 20_000.0 },
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Size => "Size",
            Self::Feedback => "Feedback",
            Self::Balance => "Balance",
            Self::PreDelayMs => "Pre-delay (ms)",
            Self::HpfHz => "HPF (Hz)",
            Self::LpfHz => "LPF (Hz)",
            Self::Chorus => "Chorus",
            Self::Send => "Send",
        }
    }

    fn read(self, p: &ReverbParams) -> f64 {
        match self {
            Self::Size => p.size as f64,
            Self::Feedback => p.feedback as f64,
            Self::Balance => p.balance as f64,
            Self::PreDelayMs => p.pre_delay_ms as f64,
            Self::HpfHz => p.hpf_hz as f64,
            Self::LpfHz => p.lpf_hz as f64,
            Self::Chorus => p.chorus as f64,
            Self::Send => p.send as f64,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Smoother {
    ramp: f32,
    target: f32,
    base: f32,
    diff: f32,
    current: f32,
}

impl Smoother {
    fn set_target(&mut self, new_target: f32) {
        if self.target != new_target {
            self.target = new_target;
            self.base = self.current;
            self.diff = new_target - self.current;
            self.ramp = 0.0;
        }
    }

    #[inline]
    fn tick(&mut self, increment: f32) -> f32 {
        if self.ramp > 1.0 {
            self.current = self.target;
        } else {
            self.current = self.base + self.diff * self.ramp;
            self.ramp += increment;
        }
        self.current
    }
}

pub struct Reverb {
    pub out_l: f32,
    pub out_r: f32,
    pub out_dry: f32,
    pub out_wet: f32,

    params: ReverbParams,

    sample_rate: f32,

    slow_tick: u32,
    slow_thrsh: u32,

    mod_1a: f32,
    mod_2a: f32,
    mod_1b: f32,
    mod_2b: f32,
    lfo_omega_1: f32,
    lfo_omega_2: f32,
    lfo_state_1: f32,
    lfo_state_2: f32,
    depth: f32,

    warp_const_pi: f32,
    omega_clip_max: f32,

    lp_a0: f32,
    lp_a1: f32,
    lp_omega: f32,
    hp_a0: f32,
    hp_a1: f32,
    hp_omega: f32,

    lp_state_l: f32,
    lp_state_r: f32,
    hp_state_l: f32,
    hp_state_r: f32,

    fb_amnt: f32,
    absorb: f32,
    bal_half: f32,
    bal_full: f32,

    buffer_indx: u32,
    buffer_sz_m1: u32,
    buffer_sz_m2: u32,

    pre_del_l: f32,
    pre_del_r: f32,

    pre_buf_l: Vec<f32>,
    pre_buf_r: Vec<f32>,
    delay_l: [Vec<f32>; 9],
    delay_r: [Vec<f32>; 9],
    state_l: [f32; 9],
    state_r: [f32; 9],

    smooth_inc: f32,

    smooth_size: Smoother,
    smooth_bal: Smoother,
    smooth_depth: Smoother,
    smooth_pre_del_l: Smoother,
    smooth_pre_del_r: Smoother,
    smooth_lp_omega: Smoother,
    smooth_hp_omega: Smoother,
}

impl Reverb {
    pub fn new(sample_rate: f32, upsample_factor: u32) -> Self {
        let buffer_size = REVERB_BUFFER_SIZE * upsample_factor as usize;
        let buffer_sz_m1 = buffer_size as u32 - 1;
        let buffer_sz_m2 = buffer_size as u32 - 2;
        let smooth_inc = 1.0 / (50.0 * (0.001 * sample_rate / 2.0)).max(1e-12);

        let mut reverb = Self {
            out_l: 0.0,
            out_r: 0.0,
            out_dry: 0.0,
            out_wet: 0.0,

            params: ReverbParams::default(),

            sample_rate,

            slow_tick: 0,
            slow_thrsh: 2 * upsample_factor - 1,

            mod_1a: 0.0,
            mod_2a: 0.0,
            mod_1b: 0.0,
            mod_2b: 0.0,
            lfo_omega_1: 0.86306 * 2.0 / sample_rate,
            lfo_omega_2: 0.6666 * 2.0 / sample_rate,
            lfo_state_1: 0.0,
            lfo_state_2: 0.0,
            depth: 0.0,

            warp_const_pi: PI / sample_rate,
            omega_clip_max: sample_rate / 2.0,

            lp_a0: 0.0,
            lp_a1: 0.0,
            lp_omega: 0.0,
            hp_a0: 0.0,
            hp_a1: 0.0,
            hp_omega: 0.0,

            lp_state_l: 0.0,
            lp_state_r: 0.0,
            hp_state_l: 0.0,
            hp_state_r: 0.0,

            fb_amnt: 0.0,
            absorb: 0.0,
            bal_half: 0.0,
            bal_full: 0.0,

            buffer_indx: 0,
            buffer_sz_m1,
            buffer_sz_m2,

            pre_del_l: 0.0,
            pre_del_r: 0.0,

            pre_buf_l: vec![0.0; buffer_size],
            pre_buf_r: vec![0.0; buffer_size],
            delay_l: std::array::from_fn(|_| vec![0.0; buffer_size]),
            delay_r: std::array::from_fn(|_| vec![0.0; buffer_size]),
            state_l: [0.0; 9],
            state_r: [0.0; 9],

            smooth_inc,

            smooth_size: Smoother::default(),
            smooth_bal: Smoother::default(),
            smooth_depth: Smoother::default(),
            smooth_pre_del_l: Smoother::default(),
            smooth_pre_del_r: Smoother::default(),
            smooth_lp_omega: Smoother::default(),
            smooth_hp_omega: Smoother::default(),
        };
        reverb.refresh_smoothers();
        reverb
    }

    pub fn params(&self) -> &ReverbParams {
        &self.params
    }

    pub fn set_params(&mut self, params: ReverbParams) {
        self.params = params;
        self.refresh_smoothers();
    }

    pub fn update_param(&mut self, param: ReverbParam) {
        match param {
            ReverbParam::Size(v) => self.params.size = v as f32,
            ReverbParam::Feedback(v) => self.params.feedback = v as f32,
            ReverbParam::Balance(v) => self.params.balance = v as f32,
            ReverbParam::PreDelayMs(v) => self.params.pre_delay_ms = v as f32,
            ReverbParam::HpfHz(v) => self.params.hpf_hz = v as f32,
            ReverbParam::LpfHz(v) => self.params.lpf_hz = v as f32,
            ReverbParam::Chorus(v) => self.params.chorus = v as f32,
            ReverbParam::Send(v) => self.params.send = v as f32,
        }
        self.refresh_smoothers();
    }

    fn refresh_smoothers(&mut self) {
        let depth_target = self.params.chorus * (self.params.size * -200.0 + 311.0);
        self.smooth_depth.set_target(depth_target);

        let size_target = self.params.size * (0.5 - self.params.size.abs() * -0.5);
        self.smooth_size.set_target(size_target);

        self.smooth_bal.set_target(self.params.balance);

        let pre_delay_samples = self.params.pre_delay_ms * 0.001 * self.sample_rate;
        self.smooth_pre_del_l.set_target(pre_delay_samples.round());
        self.smooth_pre_del_r
            .set_target((pre_delay_samples * 1.18933).round());

        let lpf = self.params.lpf_hz.clamp(0.1, self.omega_clip_max);
        self.smooth_lp_omega.set_target(tan(lpf * self.warp_const_pi));

        let hpf = self.params.hpf_hz.clamp(0.1, self.omega_clip_max);
        self.smooth_hp_omega.set_target(tan(hpf * self.warp_const_pi));
    }

    pub fn reset(&mut self) {
        self.lp_state_l = 0.0;
        self.lp_state_r = 0.0;
        self.hp_state_l = 0.0;
        self.hp_state_r = 0.0;

        self.pre_buf_l.fill(0.0);
        self.pre_buf_r.fill(0.0);
        for buf in self.delay_l.iter_mut() {
            buf.fill(0.0);
        }
        for buf in self.delay_r.iter_mut() {
            buf.fill(0.0);
        }
        self.state_l = [0.0; 9];
        self.state_r = [0.0; 9];
    }

    pub fn apply(&mut self, raw_l: f32, raw_r: f32) {
        if self.slow_tick == 0 {
            self.depth = self.smooth_depth.tick(self.smooth_inc);

            let size = self.smooth_size.tick(self.smooth_inc);
            self.absorb = size * 0.334 + 0.666;
            self.fb_amnt = size * 0.667 + 0.333;

            let bal = self.smooth_bal.tick(self.smooth_inc);
            self.bal_full = bal * (2.0 - bal);
            let bal_inv = 1.0 - bal;
            self.bal_half = bal_inv * (2.0 - bal_inv);

            self.pre_del_l = self.smooth_pre_del_l.tick(self.smooth_inc);
            self.pre_del_r = self.smooth_pre_del_r.tick(self.smooth_inc);

            self.lp_omega = self.smooth_lp_omega.tick(self.smooth_inc);
            self.lp_a0 = 1.0 / (self.lp_omega + 1.0);
            self.lp_a1 = self.lp_omega - 1.0;

            self.hp_omega = self.smooth_hp_omega.tick(self.smooth_inc);
            self.hp_a0 = 1.0 / (self.hp_omega + 1.0);
            self.hp_a1 = self.hp_omega - 1.0;

            let mut t = self.lfo_state_1 + self.lfo_omega_1;
            t -= t.round();
            self.lfo_state_1 = t;
            t = (8.0 - t.abs() * 16.0) * t + 1.0;
            self.mod_1a = t * self.depth;
            self.mod_2a = (1.0 - t) * self.depth;

            let mut t = self.lfo_state_2 + self.lfo_omega_2;
            t -= t.round();
            self.lfo_state_2 = t;
            t = (8.0 - t.abs() * 16.0) * t + 1.0;
            self.mod_1b = t * self.depth;
            self.mod_2b = (1.0 - t) * self.depth;
        }

        self.slow_tick = (self.slow_tick + 1) & self.slow_thrsh;

        let bi = self.buffer_indx as i32;
        let mask = self.buffer_sz_m1 as i32;

        // ---------------- Left channel ----------------
        let mut wet_l = raw_l * self.params.send * self.params.feedback;

        // Pre-delay L (linear-interpolated tap from pre_buf_l)
        self.pre_buf_l[self.buffer_indx as usize] = wet_l;
        let pre_l = self.pre_del_l.clamp(0.0, self.buffer_sz_m1 as f32);
        let i0 = (pre_l - 0.5).round() as i32;
        let frac = pre_l - i0 as f32;
        let im1 = i0 + 1;
        let i0_idx = ((bi - i0) & mask) as usize;
        let im1_idx = ((bi - im1) & mask) as usize;
        wet_l = self.pre_buf_l[i0_idx]
            + frac * (self.pre_buf_l[im1_idx] - self.pre_buf_l[i0_idx]);

        wet_l += self.state_r[8] * self.fb_amnt;

        // Loop filter L
        wet_l = (wet_l - self.lp_state_l * self.lp_a1) * self.lp_a0;
        let mut prev = self.lp_state_l;
        self.lp_state_l = wet_l;
        wet_l = (wet_l + prev) * self.lp_omega;

        wet_l = (wet_l - self.hp_state_l * self.hp_a1) * self.hp_a0;
        prev = self.hp_state_l;
        self.hp_state_l = wet_l;
        wet_l -= prev;

        // Allpass L1 (4-point modulated)
        wet_l = allpass_4p(
            wet_l,
            &mut self.state_l[0],
            &mut self.delay_l[0],
            bi,
            mask,
            self.buffer_sz_m2,
            self.absorb,
            REV_DEL_L1,
            self.mod_2a,
            REV_G_1,
        );

        // Allpass L2..L3 (1-point)
        wet_l = allpass_1p(
            wet_l,
            &mut self.state_l[1],
            &mut self.delay_l[1],
            bi,
            mask,
            self.absorb,
            REV_DEL_L2,
            REV_G_2,
        );
        wet_l = allpass_1p(
            wet_l,
            &mut self.state_l[2],
            &mut self.delay_l[2],
            bi,
            mask,
            self.absorb,
            REV_DEL_L3,
            REV_G_3,
        );

        // Allpass L4 — branch point: wet_l2 is the early-tap output
        let wet_l2 = allpass_1p(
            wet_l,
            &mut self.state_l[3],
            &mut self.delay_l[3],
            bi,
            mask,
            self.absorb,
            REV_DEL_L4,
            REV_G_4,
        );

        // Allpass L5 starts from wet_l2
        wet_l = allpass_1p(
            wet_l2,
            &mut self.state_l[4],
            &mut self.delay_l[4],
            bi,
            mask,
            self.absorb,
            REV_DEL_L5,
            REV_G_4,
        );
        wet_l = allpass_1p(
            wet_l,
            &mut self.state_l[5],
            &mut self.delay_l[5],
            bi,
            mask,
            self.absorb,
            REV_DEL_L6,
            REV_G_4,
        );
        wet_l = allpass_1p(
            wet_l,
            &mut self.state_l[6],
            &mut self.delay_l[6],
            bi,
            mask,
            self.absorb,
            REV_DEL_L7,
            REV_G_4,
        );
        wet_l = allpass_1p(
            wet_l,
            &mut self.state_l[7],
            &mut self.delay_l[7],
            bi,
            mask,
            self.absorb,
            REV_DEL_L8,
            REV_G_4,
        );

        // ---------------- Right channel ----------------
        let mut wet_r = raw_r * self.params.send * self.params.feedback;

        self.pre_buf_r[self.buffer_indx as usize] = wet_r;
        let pre_r = self.pre_del_r.clamp(0.0, self.buffer_sz_m1 as f32);
        let i0 = (pre_r - 0.5).round() as i32;
        let frac = pre_r - i0 as f32;
        let im1 = i0 + 1;
        let i0_idx = ((bi - i0) & mask) as usize;
        let im1_idx = ((bi - im1) & mask) as usize;
        wet_r = self.pre_buf_r[i0_idx]
            + frac * (self.pre_buf_r[im1_idx] - self.pre_buf_r[i0_idx]);

        wet_r += self.state_l[8] * self.fb_amnt;

        wet_r = (wet_r - self.lp_state_r * self.lp_a1) * self.lp_a0;
        let mut prev = self.lp_state_r;
        self.lp_state_r = wet_r;
        wet_r = (wet_r + prev) * self.lp_omega;

        wet_r = (wet_r - self.hp_state_r * self.hp_a1) * self.hp_a0;
        prev = self.hp_state_r;
        self.hp_state_r = wet_r;
        wet_r -= prev;

        wet_r = allpass_4p(
            wet_r,
            &mut self.state_r[0],
            &mut self.delay_r[0],
            bi,
            mask,
            self.buffer_sz_m2,
            self.absorb,
            REV_DEL_R1,
            self.mod_2b,
            REV_G_1,
        );

        wet_r = allpass_1p(
            wet_r,
            &mut self.state_r[1],
            &mut self.delay_r[1],
            bi,
            mask,
            self.absorb,
            REV_DEL_R2,
            REV_G_2,
        );
        wet_r = allpass_1p(
            wet_r,
            &mut self.state_r[2],
            &mut self.delay_r[2],
            bi,
            mask,
            self.absorb,
            REV_DEL_R3,
            REV_G_3,
        );

        let wet_r2 = allpass_1p(
            wet_r,
            &mut self.state_r[3],
            &mut self.delay_r[3],
            bi,
            mask,
            self.absorb,
            REV_DEL_R4,
            REV_G_4,
        );
        wet_r = allpass_1p(
            wet_r2,
            &mut self.state_r[4],
            &mut self.delay_r[4],
            bi,
            mask,
            self.absorb,
            REV_DEL_R5,
            REV_G_4,
        );
        wet_r = allpass_1p(
            wet_r,
            &mut self.state_r[5],
            &mut self.delay_r[5],
            bi,
            mask,
            self.absorb,
            REV_DEL_R6,
            REV_G_4,
        );
        wet_r = allpass_1p(
            wet_r,
            &mut self.state_r[6],
            &mut self.delay_r[6],
            bi,
            mask,
            self.absorb,
            REV_DEL_R7,
            REV_G_4,
        );
        wet_r = allpass_1p(
            wet_r,
            &mut self.state_r[7],
            &mut self.delay_r[7],
            bi,
            mask,
            self.absorb,
            REV_DEL_R8,
            REV_G_4,
        );

        // ---------------- Cross-feedback delays L9 / R9 ----------------
        self.delay_l[8][self.buffer_indx as usize] = wet_l;
        self.state_l[8] = read_4p_modulated(
            &self.delay_l[8],
            bi,
            mask,
            self.buffer_sz_m2,
            REV_DEL_L9,
            self.mod_1a,
        );

        self.delay_r[8][self.buffer_indx as usize] = wet_r;
        self.state_r[8] = read_4p_modulated(
            &self.delay_r[8],
            bi,
            mask,
            self.buffer_sz_m2,
            REV_DEL_R9,
            self.mod_1b,
        );

        self.buffer_indx = (self.buffer_indx + 1) & self.buffer_sz_m1;

        // ---------------- Output (wet only — dry/wet mix happens in ChannelMixer) ----------------
        let mix_l = wet_l * self.bal_full + wet_l2 * self.bal_half;
        let mix_r = wet_r * self.bal_full + wet_r2 * self.bal_half;

        self.out_l = mix_l;
        self.out_r = mix_r;

        self.out_dry = raw_l + raw_r;
        self.out_wet = mix_l + mix_r;
    }
}

#[inline]
fn allpass_1p(
    wet_in: f32,
    state: &mut f32,
    buffer: &mut [f32],
    buffer_indx: i32,
    mask: i32,
    absorb: f32,
    delay: u32,
    gain: f32,
) -> f32 {
    let tmp = *state * absorb;
    let stored = wet_in + tmp * gain;
    buffer[buffer_indx as usize] = stored;
    let allpass_out = stored * -gain + tmp;
    let ind = ((buffer_indx - delay as i32) & mask) as usize;
    *state = buffer[ind];
    allpass_out
}

#[inline]
fn allpass_4p(
    wet_in: f32,
    state: &mut f32,
    buffer: &mut [f32],
    buffer_indx: i32,
    mask: i32,
    buffer_sz_m2: u32,
    absorb: f32,
    delay_base: u32,
    mod_amount: f32,
    gain: f32,
) -> f32 {
    let tmp = *state * absorb;
    let stored = wet_in + tmp * gain;
    buffer[buffer_indx as usize] = stored;
    let allpass_out = stored * -gain + tmp;

    let target = (delay_base as f32 + mod_amount).clamp(1.0, buffer_sz_m2 as f32);
    let i0 = (target - 0.5).round() as i32;
    let frac = target - i0 as f32;
    let im1 = i32::max(i0, 1) - 1;
    let ip1 = i0 + 1;
    let ip2 = i0 + 2;

    let im1_idx = ((buffer_indx - im1) & mask) as usize;
    let i0_idx = ((buffer_indx - i0) & mask) as usize;
    let ip1_idx = ((buffer_indx - ip1) & mask) as usize;
    let ip2_idx = ((buffer_indx - ip2) & mask) as usize;

    *state = interpol_rt(
        frac,
        buffer[im1_idx],
        buffer[i0_idx],
        buffer[ip1_idx],
        buffer[ip2_idx],
    );
    allpass_out
}

#[inline]
fn read_4p_modulated(
    buffer: &[f32],
    buffer_indx: i32,
    mask: i32,
    buffer_sz_m2: u32,
    delay_base: u32,
    mod_amount: f32,
) -> f32 {
    let target = (delay_base as f32 + mod_amount).clamp(0.0, buffer_sz_m2 as f32);
    let i0 = (target - 0.5).round() as i32;
    let frac = target - i0 as f32;
    let im1 = i32::max(i0, 1) - 1;
    let ip1 = i0 + 1;
    let ip2 = i0 + 2;

    let im1_idx = ((buffer_indx - im1) & mask) as usize;
    let i0_idx = ((buffer_indx - i0) & mask) as usize;
    let ip1_idx = ((buffer_indx - ip1) & mask) as usize;
    let ip2_idx = ((buffer_indx - ip2) & mask) as usize;

    interpol_rt(
        frac,
        buffer[im1_idx],
        buffer[i0_idx],
        buffer[ip1_idx],
        buffer[ip2_idx],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_params() -> ReverbParams {
        ReverbParams {
            size: 0.5,
            feedback: 1.0,
            balance: 0.5,
            pre_delay_ms: 5.0,
            hpf_hz: 200.0,
            lpf_hz: 8000.0,
            chorus: 0.5,
            send: 1.0,
        }
    }

    fn warm_up(reverb: &mut Reverb, samples: usize) {
        for _ in 0..samples {
            reverb.apply(0.0, 0.0);
        }
    }

    #[test]
    fn finite_with_silence_input() {
        let mut reverb = Reverb::new(48_000.0, 1);
        reverb.set_params(test_params());
        for _ in 0..96_000 {
            reverb.apply(0.0, 0.0);
            assert!(reverb.out_l.is_finite(), "left went non-finite");
            assert!(reverb.out_r.is_finite(), "right went non-finite");
        }
    }

    #[test]
    fn impulse_response_decays() {
        let mut reverb = Reverb::new(48_000.0, 1);
        reverb.set_params(test_params());
        warm_up(&mut reverb, 48_000);

        reverb.apply(1.0, 1.0);

        let mut peak_late = 0.0f32;
        let one_sec = 48_000;
        for i in 0..(2 * one_sec) {
            reverb.apply(0.0, 0.0);
            assert!(reverb.out_l.is_finite() && reverb.out_r.is_finite());
            if i >= one_sec {
                peak_late = peak_late.max(reverb.out_l.abs()).max(reverb.out_r.abs());
            }
        }
        assert!(
            peak_late < 0.1,
            "tail did not decay enough after 1s, peak={peak_late}"
        );
    }

    #[test]
    fn bounded_output_for_bounded_sine() {
        let mut reverb = Reverb::new(48_000.0, 1);
        reverb.set_params(test_params());
        warm_up(&mut reverb, 48_000);

        let mut peak = 0.0f32;
        for i in 0..96_000 {
            let phase = (i as f32) * 2.0 * std::f32::consts::PI * 440.0 / 48_000.0;
            let s = phase.sin() * 0.5;
            reverb.apply(s, s);
            peak = peak.max(reverb.out_l.abs()).max(reverb.out_r.abs());
        }
        assert!(peak.is_finite());
        assert!(peak < 5.0, "peak too large: {peak}");
    }

    #[test]
    fn out_l_r_match_out_wet_split() {
        let mut reverb = Reverb::new(48_000.0, 1);
        reverb.set_params(test_params());
        warm_up(&mut reverb, 48_000);

        reverb.apply(0.5, 0.5);
        assert!(
            (reverb.out_wet - (reverb.out_l + reverb.out_r)).abs() < 1e-6,
            "out_wet should equal out_l + out_r"
        );
    }

    #[test]
    fn out_dry_and_wet_sums_are_raw() {
        let mut reverb = Reverb::new(48_000.0, 1);
        reverb.set_params(test_params());
        warm_up(&mut reverb, 48_000);

        reverb.apply(0.4, -0.2);
        assert!(
            (reverb.out_dry - (0.4 + -0.2)).abs() < 1e-6,
            "out_dry should be raw sum, got {}",
            reverb.out_dry
        );
        assert!(reverb.out_wet.is_finite());
    }
}
