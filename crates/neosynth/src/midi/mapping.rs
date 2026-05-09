use crate::audio::InputParameters;
use crate::dsp::echo::EchoParamKind;
use crate::dsp::mixer::{self, MixerBoolId, MixerFloatId, MixerParamId};
use crate::dsp::param::{FloatCurve, FloatParams};
use crate::dsp::reverb::ReverbParamKind;

/// What float-typed parameter a CC binding addresses.
///
/// Adding a new effect = one new variant here + one arm in `into_param`. The
/// effect's *list of parameters* is owned by the effect itself (its
/// `*ParamKind` enum + `FloatParams::all()`); this enum just dispatches.
#[derive(Debug, Clone, Copy)]
pub enum FloatTarget {
    Reverb(ReverbParamKind),
    Echo(EchoParamKind),
    Mixer(MixerFloatId),
}

impl FloatTarget {
    pub fn into_param(self, v: f64) -> InputParameters {
        match self {
            Self::Reverb(id) => InputParameters::Reverb(id.build(v)),
            Self::Echo(id) => InputParameters::Echo(id.build(v)),
            Self::Mixer(id) => InputParameters::Mixer(id.build(v)),
        }
    }
}

/// What bool-typed parameter a CC binding addresses. Today only mixer mutes /
/// pre-fader switches are bool-typed, so the enum has a single variant.
#[derive(Debug, Clone, Copy)]
pub enum BoolTarget {
    Mixer(MixerBoolId),
}

impl BoolTarget {
    pub fn into_param(self, v: bool) -> InputParameters {
        match self {
            Self::Mixer(id) => InputParameters::Mixer(id.build(v)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BoolMode {
    /// Controller maintains state and sends 0 or 127 directly.
    /// `value >= 64` → true.
    Latched,
    /// Controller sends a momentary value; we toggle our internal state on
    /// the rising edge (low → high crossing of `threshold`).
    /// Reserved for momentary footswitches / pads — not yet wired to a binding.
    #[allow(dead_code)]
    Toggle { threshold: u8 },
}

#[derive(Debug, Clone, Copy)]
pub enum CcBinding {
    Float {
        channel: u8,
        cc: u8,
        target: FloatTarget,
        curve: FloatCurve,
    },
    Bool {
        channel: u8,
        cc: u8,
        target: BoolTarget,
        mode: BoolMode,
    },
}

/// Build the default mapping: channel 0, CCs starting at 1, mixer first
/// (per-strip block), then reverb edit knobs, then echo edit knobs.
///
/// Each effect declares its own parameter list — this function iterates them
/// and never enumerates parameters by hand. Adding a parameter to an effect
/// only requires updating that effect's `*ParamKind::all()`.
pub fn default_mapping(num_inputs: usize) -> Vec<CcBinding> {
    let channel: u8 = 0;
    let mut bindings = Vec::new();
    let mut cc_num: u8 = 0;

    // Mixer — per-strip and FX returns + master, in display order.
    for id in mixer::default_param_order(num_inputs) {
        cc_num += 1;
        match id {
            MixerParamId::Float(f) => bindings.push(CcBinding::Float {
                channel,
                cc: cc_num,
                target: FloatTarget::Mixer(f),
                curve: f.default_curve(),
            }),
            MixerParamId::Bool(b) => bindings.push(CcBinding::Bool {
                channel,
                cc: cc_num,
                target: BoolTarget::Mixer(b),
                mode: BoolMode::Latched,
            }),
        }
    }

    append_float_params::<ReverbParamKind, _>(&mut bindings, &mut cc_num, channel, FloatTarget::Reverb);
    append_float_params::<EchoParamKind, _>(&mut bindings, &mut cc_num, channel, FloatTarget::Echo);

    bindings
}

/// Generic helper: append one CC float-binding per parameter exposed by `K`.
fn append_float_params<K: FloatParams, F: Fn(K) -> FloatTarget>(
    bindings: &mut Vec<CcBinding>,
    cc: &mut u8,
    channel: u8,
    wrap: F,
) {
    for &id in K::all() {
        *cc += 1;
        bindings.push(CcBinding::Float {
            channel,
            cc: *cc,
            target: wrap(id),
            curve: id.default_curve(),
        });
    }
}
