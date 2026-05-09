use crate::dsp::dsp_toolbox::constants::TWO_PI;
use crate::dsp::param::FloatCurve;
use crate::dsp::utils::db_to_linear;

#[derive(Debug, Clone, Copy)]
pub struct InputStripParams {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub send_reverb: f32,
    pub send_echo: f32,
    pub send_pre_fader: bool,
}

impl Default for InputStripParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
            send_reverb: 0.0,
            send_echo: 0.0,
            send_pre_fader: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FxReturnParams {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
}

impl Default for FxReturnParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MasterParams {
    pub gain: f32,
}

impl Default for MasterParams {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MixerParam {
    InputGainDb(usize, f64),
    InputPan(usize, f64),
    InputMute(usize, bool),
    InputSendReverb(usize, f64),
    InputSendEcho(usize, f64),
    InputSendPreFader(usize, bool),

    ReverbReturnGainDb(f64),
    ReverbReturnPan(f64),
    ReverbReturnMute(bool),

    EchoReturnGainDb(f64),
    EchoReturnPan(f64),
    EchoReturnMute(bool),

    MasterGainDb(f64),
}

/// Float-typed mixer targets (kind only, no value). Strip-indexed targets
/// carry the strip index because the mixer is a single component covering
/// many strips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerFloatId {
    InputGainDb(usize),
    InputPan(usize),
    InputSendReverb(usize),
    InputSendEcho(usize),
    ReverbReturnGainDb,
    ReverbReturnPan,
    EchoReturnGainDb,
    EchoReturnPan,
    MasterGainDb,
}

impl MixerFloatId {
    pub fn build(self, v: f64) -> MixerParam {
        match self {
            Self::InputGainDb(i) => MixerParam::InputGainDb(i, v),
            Self::InputPan(i) => MixerParam::InputPan(i, v),
            Self::InputSendReverb(i) => MixerParam::InputSendReverb(i, v),
            Self::InputSendEcho(i) => MixerParam::InputSendEcho(i, v),
            Self::ReverbReturnGainDb => MixerParam::ReverbReturnGainDb(v),
            Self::ReverbReturnPan => MixerParam::ReverbReturnPan(v),
            Self::EchoReturnGainDb => MixerParam::EchoReturnGainDb(v),
            Self::EchoReturnPan => MixerParam::EchoReturnPan(v),
            Self::MasterGainDb => MixerParam::MasterGainDb(v),
        }
    }

    pub fn default_curve(self) -> FloatCurve {
        match self {
            Self::InputGainDb(_)
            | Self::ReverbReturnGainDb
            | Self::EchoReturnGainDb
            | Self::MasterGainDb => FloatCurve::Linear { min: -60.0, max: 12.0 },
            Self::InputPan(_) | Self::ReverbReturnPan | Self::EchoReturnPan => {
                FloatCurve::Linear { min: -1.0, max: 1.0 }
            }
            Self::InputSendReverb(_) | Self::InputSendEcho(_) => {
                FloatCurve::Linear { min: 0.0, max: 1.0 }
            }
        }
    }

    /// Default user-facing value (in the units the OSC address expects):
    /// 0.0 dB / 0.0 pan / 0.0 send, regardless of variant.
    pub fn default_value(self) -> f32 {
        0.0
    }

    /// Read this parameter's current value out of a `MixerSnapshot`. Returns
    /// `None` if the indexed strip doesn't exist.
    pub fn read(self, snap: &crate::persist::MixerSnapshot) -> Option<f32> {
        match self {
            Self::InputGainDb(i) => snap.inputs.get(i).map(|s| s.gain_db),
            Self::InputPan(i) => snap.inputs.get(i).map(|s| s.pan),
            Self::InputSendReverb(i) => snap.inputs.get(i).map(|s| s.send_reverb),
            Self::InputSendEcho(i) => snap.inputs.get(i).map(|s| s.send_echo),
            Self::ReverbReturnGainDb => Some(snap.reverb_return.gain_db),
            Self::ReverbReturnPan => Some(snap.reverb_return.pan),
            Self::EchoReturnGainDb => Some(snap.echo_return.gain_db),
            Self::EchoReturnPan => Some(snap.echo_return.pan),
            Self::MasterGainDb => Some(snap.master_gain_db),
        }
    }

    /// Full OSC address for this mixer parameter.
    pub fn osc_path(self) -> String {
        match self {
            Self::InputGainDb(i) => format!("/mixer/input/{i}/gain_db"),
            Self::InputPan(i) => format!("/mixer/input/{i}/pan"),
            Self::InputSendReverb(i) => format!("/mixer/input/{i}/send_reverb"),
            Self::InputSendEcho(i) => format!("/mixer/input/{i}/send_echo"),
            Self::ReverbReturnGainDb => "/mixer/reverb_return/gain_db".into(),
            Self::ReverbReturnPan => "/mixer/reverb_return/pan".into(),
            Self::EchoReturnGainDb => "/mixer/echo_return/gain_db".into(),
            Self::EchoReturnPan => "/mixer/echo_return/pan".into(),
            Self::MasterGainDb => "/mixer/master/gain_db".into(),
        }
    }
}

/// Bool-typed mixer targets (kind only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerBoolId {
    InputMute(usize),
    InputSendPreFader(usize),
    ReverbReturnMute,
    EchoReturnMute,
}

impl MixerBoolId {
    pub fn build(self, v: bool) -> MixerParam {
        match self {
            Self::InputMute(i) => MixerParam::InputMute(i, v),
            Self::InputSendPreFader(i) => MixerParam::InputSendPreFader(i, v),
            Self::ReverbReturnMute => MixerParam::ReverbReturnMute(v),
            Self::EchoReturnMute => MixerParam::EchoReturnMute(v),
        }
    }

    pub fn default_value(self) -> bool {
        false
    }

    pub fn read(self, snap: &crate::persist::MixerSnapshot) -> Option<bool> {
        match self {
            Self::InputMute(i) => snap.inputs.get(i).map(|s| s.mute),
            Self::InputSendPreFader(i) => snap.inputs.get(i).map(|s| s.send_pre_fader),
            Self::ReverbReturnMute => Some(snap.reverb_return.mute),
            Self::EchoReturnMute => Some(snap.echo_return.mute),
        }
    }

    pub fn osc_path(self) -> String {
        match self {
            Self::InputMute(i) => format!("/mixer/input/{i}/mute"),
            Self::InputSendPreFader(i) => format!("/mixer/input/{i}/send_pre_fader"),
            Self::ReverbReturnMute => "/mixer/reverb_return/mute".into(),
            Self::EchoReturnMute => "/mixer/echo_return/mute".into(),
        }
    }
}

/// Either a float-id or bool-id mixer target. Used to enumerate the mixer's
/// parameters in the order we want for a default control-surface mapping.
#[derive(Debug, Clone, Copy)]
pub enum MixerParamId {
    Float(MixerFloatId),
    Bool(MixerBoolId),
}

/// Returns every mixer parameter in display order (per-strip block, then FX
/// returns, then master). Strip-indexed targets are expanded for `num_inputs`.
pub fn default_param_order(num_inputs: usize) -> Vec<MixerParamId> {
    let mut out = Vec::new();
    for i in 0..num_inputs {
        out.push(MixerParamId::Float(MixerFloatId::InputGainDb(i)));
        out.push(MixerParamId::Float(MixerFloatId::InputPan(i)));
        out.push(MixerParamId::Float(MixerFloatId::InputSendReverb(i)));
        out.push(MixerParamId::Float(MixerFloatId::InputSendEcho(i)));
        out.push(MixerParamId::Bool(MixerBoolId::InputMute(i)));
        out.push(MixerParamId::Bool(MixerBoolId::InputSendPreFader(i)));
    }
    out.push(MixerParamId::Float(MixerFloatId::ReverbReturnGainDb));
    out.push(MixerParamId::Float(MixerFloatId::ReverbReturnPan));
    out.push(MixerParamId::Bool(MixerBoolId::ReverbReturnMute));
    out.push(MixerParamId::Float(MixerFloatId::EchoReturnGainDb));
    out.push(MixerParamId::Float(MixerFloatId::EchoReturnPan));
    out.push(MixerParamId::Bool(MixerBoolId::EchoReturnMute));
    out.push(MixerParamId::Float(MixerFloatId::MasterGainDb));
    out
}

/// Per-buffer level accumulators. The audio loop calls `Mixer::reset_levels()`
/// at the start of each buffer, processes samples (which update these), then
/// reads `Mixer::levels()` to derive (peak, RMS) and publish to `MetersOutput`.
///
/// `peak_*` fields track running max-abs. `sum_sq_*` fields accumulate
/// `sample²`; the engine computes RMS = sqrt(sum_sq / N) at publish time.
#[derive(Debug, Clone)]
pub struct Levels {
    pub input_peaks: Vec<f32>,
    pub input_sum_sq: Vec<f32>,
    pub reverb_peak: f32,
    pub reverb_sum_sq: f32,
    pub echo_peak: f32,
    pub echo_sum_sq: f32,
    pub master_l_peak: f32,
    pub master_l_sum_sq: f32,
    pub master_r_peak: f32,
    pub master_r_sum_sq: f32,
}

pub struct Mixer {
    pub master_l: f32,
    pub master_r: f32,

    pub reverb_bus_l: f32,
    pub reverb_bus_r: f32,
    pub echo_bus_l: f32,
    pub echo_bus_r: f32,

    inputs: Vec<InputStripParams>,
    reverb_return: FxReturnParams,
    echo_return: FxReturnParams,
    master: MasterParams,

    levels: Levels,

    hp_b0: f32,
    hp_state_l: f32,
    hp_state_r: f32,
}

impl Mixer {
    pub fn new(sample_rate: f32, num_inputs: usize) -> Self {
        let norm_omega = TWO_PI / sample_rate;
        Self {
            master_l: 0.0,
            master_r: 0.0,
            reverb_bus_l: 0.0,
            reverb_bus_r: 0.0,
            echo_bus_l: 0.0,
            echo_bus_r: 0.0,
            inputs: vec![InputStripParams::default(); num_inputs],
            reverb_return: FxReturnParams::default(),
            echo_return: FxReturnParams::default(),
            master: MasterParams::default(),
            levels: Levels {
                input_peaks: vec![0.0; num_inputs],
                input_sum_sq: vec![0.0; num_inputs],
                reverb_peak: 0.0,
                reverb_sum_sq: 0.0,
                echo_peak: 0.0,
                echo_sum_sq: 0.0,
                master_l_peak: 0.0,
                master_l_sum_sq: 0.0,
                master_r_peak: 0.0,
                master_r_sum_sq: 0.0,
            },
            hp_b0: norm_omega * 12.978,
            hp_state_l: 0.0,
            hp_state_r: 0.0,
        }
    }

    pub fn levels(&self) -> &Levels {
        &self.levels
    }

    pub fn reset_levels(&mut self) {
        for v in self.levels.input_peaks.iter_mut() {
            *v = 0.0;
        }
        for v in self.levels.input_sum_sq.iter_mut() {
            *v = 0.0;
        }
        self.levels.reverb_peak = 0.0;
        self.levels.reverb_sum_sq = 0.0;
        self.levels.echo_peak = 0.0;
        self.levels.echo_sum_sq = 0.0;
        self.levels.master_l_peak = 0.0;
        self.levels.master_l_sum_sq = 0.0;
        self.levels.master_r_peak = 0.0;
        self.levels.master_r_sum_sq = 0.0;
    }

    pub fn update_param(&mut self, param: MixerParam) {
        match param {
            MixerParam::InputGainDb(i, db) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.gain = db_to_linear(db as f32);
                }
            }
            MixerParam::InputPan(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.pan = (v as f32).clamp(-1.0, 1.0);
                }
            }
            MixerParam::InputMute(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.mute = v;
                }
            }
            MixerParam::InputSendReverb(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_reverb = (v as f32).clamp(0.0, 1.0);
                }
            }
            MixerParam::InputSendEcho(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_echo = (v as f32).clamp(0.0, 1.0);
                }
            }
            MixerParam::InputSendPreFader(i, v) => {
                if let Some(s) = self.inputs.get_mut(i) {
                    s.send_pre_fader = v;
                }
            }
            MixerParam::ReverbReturnGainDb(db) => {
                self.reverb_return.gain = db_to_linear(db as f32);
            }
            MixerParam::ReverbReturnPan(v) => {
                self.reverb_return.pan = (v as f32).clamp(-1.0, 1.0);
            }
            MixerParam::ReverbReturnMute(v) => {
                self.reverb_return.mute = v;
            }
            MixerParam::EchoReturnGainDb(db) => {
                self.echo_return.gain = db_to_linear(db as f32);
            }
            MixerParam::EchoReturnPan(v) => {
                self.echo_return.pan = (v as f32).clamp(-1.0, 1.0);
            }
            MixerParam::EchoReturnMute(v) => {
                self.echo_return.mute = v;
            }
            MixerParam::MasterGainDb(db) => {
                self.master.gain = db_to_linear(db as f32);
            }
        }
    }

    /// Reset master and FX buses, then accumulate input strips.
    /// `inputs.len()` must equal `num_inputs()`.
    #[inline]
    pub fn process_inputs(&mut self, inputs: &[f32]) {
        debug_assert_eq!(inputs.len(), self.inputs.len());

        self.master_l = 0.0;
        self.master_r = 0.0;
        self.reverb_bus_l = 0.0;
        self.reverb_bus_r = 0.0;
        self.echo_bus_l = 0.0;
        self.echo_bus_r = 0.0;

        for (i, (sample, strip)) in inputs
            .iter()
            .copied()
            .zip(self.inputs.iter())
            .enumerate()
        {
            if strip.mute {
                continue;
            }

            let (gl, gr) = mono_pan_gains(strip.pan);
            let post_fader = sample * strip.gain;
            let panned_l = post_fader * gl;
            let panned_r = post_fader * gr;

            // Post-fader pre-pan: peak (max abs) and sum_sq for RMS.
            let abs_pf = post_fader.abs();
            if abs_pf > self.levels.input_peaks[i] {
                self.levels.input_peaks[i] = abs_pf;
            }
            self.levels.input_sum_sq[i] += post_fader * post_fader;

            self.master_l += panned_l;
            self.master_r += panned_r;

            let (send_l, send_r) = if strip.send_pre_fader {
                (sample * gl, sample * gr)
            } else {
                (panned_l, panned_r)
            };

            self.reverb_bus_l += send_l * strip.send_reverb;
            self.reverb_bus_r += send_r * strip.send_reverb;
            self.echo_bus_l += send_l * strip.send_echo;
            self.echo_bus_r += send_r * strip.send_echo;
        }
    }

    /// Add panned + gained FX returns to the master sum.
    #[inline]
    pub fn add_returns(&mut self, reverb_l: f32, reverb_r: f32, echo_l: f32, echo_r: f32) {
        if !self.reverb_return.mute {
            let (gl, gr) = stereo_balance_gains(self.reverb_return.pan);
            let g = self.reverb_return.gain;
            let post_l = reverb_l * g * gl;
            let post_r = reverb_r * g * gr;
            self.master_l += post_l;
            self.master_r += post_r;
            let p = post_l.abs().max(post_r.abs());
            if p > self.levels.reverb_peak {
                self.levels.reverb_peak = p;
            }
            self.levels.reverb_sum_sq += post_l * post_l + post_r * post_r;
        }
        if !self.echo_return.mute {
            let (gl, gr) = stereo_balance_gains(self.echo_return.pan);
            let g = self.echo_return.gain;
            let post_l = echo_l * g * gl;
            let post_r = echo_r * g * gr;
            self.master_l += post_l;
            self.master_r += post_r;
            let p = post_l.abs().max(post_r.abs());
            if p > self.levels.echo_peak {
                self.levels.echo_peak = p;
            }
            self.levels.echo_sum_sq += post_l * post_l + post_r * post_r;
        }
    }

    /// Apply master gain + DC blocker. Result lives in `self.master_l/master_r`.
    #[inline]
    pub fn finalize(&mut self) {
        let m_l = self.master_l * self.master.gain;
        let m_r = self.master_r * self.master.gain;

        let hp_l = m_l - self.hp_state_l;
        self.hp_state_l = hp_l * self.hp_b0 + self.hp_state_l;
        let hp_r = m_r - self.hp_state_r;
        self.hp_state_r = hp_r * self.hp_b0 + self.hp_state_r;

        self.master_l = hp_l;
        self.master_r = hp_r;

        let abs_l = hp_l.abs();
        let abs_r = hp_r.abs();
        if abs_l > self.levels.master_l_peak {
            self.levels.master_l_peak = abs_l;
        }
        if abs_r > self.levels.master_r_peak {
            self.levels.master_r_peak = abs_r;
        }
        self.levels.master_l_sum_sq += hp_l * hp_l;
        self.levels.master_r_sum_sq += hp_r * hp_r;
    }
}

/// Equal-power pan for a mono source going to a stereo bus.
/// pan=-1 → (1,0), pan=0 → (0.707, 0.707), pan=+1 → (0,1).
#[inline]
fn mono_pan_gains(pan: f32) -> (f32, f32) {
    let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (theta.cos(), theta.sin())
}

/// Balance law for a stereo source.
/// pan=0 → (1,1) identity, pan=-1 → (1,0), pan=+1 → (0,1).
#[inline]
fn stereo_balance_gains(pan: f32) -> (f32, f32) {
    if pan <= 0.0 {
        (1.0, 1.0 + pan)
    } else {
        (1.0 - pan, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac_peak_after_settle<F: FnMut(f32) -> f32>(mut step: F) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..10_000 {
            let sign = if i & 1 == 0 { 1.0 } else { -1.0 };
            let v = step(sign);
            if i >= 1_000 {
                peak = peak.max(v.abs());
            }
        }
        peak
    }

    #[test]
    fn mono_pan_center_is_equal_power() {
        let (l, r) = mono_pan_gains(0.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l * l + r * r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mono_pan_extremes() {
        let (l, r) = mono_pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        let (l, r) = mono_pan_gains(1.0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stereo_balance_center_is_identity() {
        let (l, r) = stereo_balance_gains(0.0);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn input_panned_left_only_routes_to_left() {
        let mut mixer = Mixer::new(48_000.0, 1);
        mixer.update_param(MixerParam::InputPan(0, -1.0));
        mixer.update_param(MixerParam::InputGainDb(0, 0.0));

        let peak_l = ac_peak_after_settle(|s| {
            mixer.process_inputs(&[s * 0.5]);
            mixer.add_returns(0.0, 0.0, 0.0, 0.0);
            mixer.finalize();
            mixer.master_l
        });
        assert!((peak_l - 0.5).abs() < 1e-2, "peak_l={peak_l}");

        let mut mixer = Mixer::new(48_000.0, 1);
        mixer.update_param(MixerParam::InputPan(0, -1.0));
        let peak_r = ac_peak_after_settle(|s| {
            mixer.process_inputs(&[s * 0.5]);
            mixer.add_returns(0.0, 0.0, 0.0, 0.0);
            mixer.finalize();
            mixer.master_r
        });
        assert!(peak_r.abs() < 1e-3, "peak_r={peak_r}");
    }

    #[test]
    fn mute_silences_strip() {
        let mut mixer = Mixer::new(48_000.0, 1);
        mixer.update_param(MixerParam::InputMute(0, true));

        for _ in 0..1000 {
            mixer.process_inputs(&[1.0]);
            mixer.add_returns(0.0, 0.0, 0.0, 0.0);
            mixer.finalize();
        }
        assert!(mixer.master_l.abs() < 1e-6);
        assert!(mixer.master_r.abs() < 1e-6);
    }

    #[test]
    fn post_fader_send_scales_with_strip_gain() {
        let mut mixer = Mixer::new(48_000.0, 1);
        // -6 dB ≈ 0.501
        mixer.update_param(MixerParam::InputGainDb(0, -6.0));
        mixer.update_param(MixerParam::InputPan(0, 0.0));
        mixer.update_param(MixerParam::InputSendReverb(0, 1.0));
        mixer.update_param(MixerParam::InputSendPreFader(0, false));

        mixer.process_inputs(&[1.0]);
        // Pan center → 0.707; gain -6dB → ~0.501; total ~0.354 per side
        assert!(
            (mixer.reverb_bus_l - 0.354).abs() < 0.01,
            "reverb_bus_l={}",
            mixer.reverb_bus_l
        );
    }

    #[test]
    fn pre_fader_send_independent_of_strip_gain() {
        let mut mixer = Mixer::new(48_000.0, 1);
        mixer.update_param(MixerParam::InputGainDb(0, -60.0)); // basically silent post-fader
        mixer.update_param(MixerParam::InputPan(0, 0.0));
        mixer.update_param(MixerParam::InputSendReverb(0, 1.0));
        mixer.update_param(MixerParam::InputSendPreFader(0, true));

        mixer.process_inputs(&[1.0]);
        // Pre-fader: send sees 1.0 * pan(0) = 0.707 even though strip is -60 dB
        assert!(
            (mixer.reverb_bus_l - 0.707).abs() < 0.01,
            "reverb_bus_l={}",
            mixer.reverb_bus_l
        );
    }

    #[test]
    fn master_gain_db_scales_output() {
        let mut mixer = Mixer::new(48_000.0, 1);
        mixer.update_param(MixerParam::InputPan(0, -1.0));
        mixer.update_param(MixerParam::MasterGainDb(-6.0)); // ~0.501x

        let peak = ac_peak_after_settle(|s| {
            mixer.process_inputs(&[s]);
            mixer.add_returns(0.0, 0.0, 0.0, 0.0);
            mixer.finalize();
            mixer.master_l
        });
        // Input pan-left routes 1.0 to L at unity, master -6 dB → ~0.501
        assert!((peak - 0.501).abs() < 0.02, "peak={peak}");
    }

    #[test]
    fn fx_returns_pan_as_balance() {
        let mut mixer = Mixer::new(48_000.0, 0);
        mixer.update_param(MixerParam::ReverbReturnPan(0.0)); // identity at center
        let peak_l = ac_peak_after_settle(|s| {
            mixer.process_inputs(&[]);
            mixer.add_returns(s * 0.5, s * 0.5, 0.0, 0.0);
            mixer.finalize();
            mixer.master_l
        });
        assert!((peak_l - 0.5).abs() < 1e-2, "peak_l={peak_l}");
    }

    #[test]
    fn dc_blocker_kills_constant_offset() {
        let mut mixer = Mixer::new(48_000.0, 1);
        for _ in 0..480_000 {
            mixer.process_inputs(&[0.5]);
            mixer.add_returns(0.0, 0.0, 0.0, 0.0);
            mixer.finalize();
        }
        assert!(mixer.master_l.abs() < 1e-3, "master_l={}", mixer.master_l);
    }
}
