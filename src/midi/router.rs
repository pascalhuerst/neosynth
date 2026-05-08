use crate::audio::InputParameters;

use super::mapping::{BoolMode, CcBinding};

#[derive(Default, Clone, Copy)]
struct RuntimeState {
    last_cc_value: u8,
    toggle_state: bool,
}

/// Routes incoming CC events to `InputParameters`. Holds per-binding state
/// for `BoolMode::Toggle` edge detection and toggle latch.
pub struct CcRouter {
    bindings: Vec<CcBinding>,
    state: Vec<RuntimeState>,
}

impl CcRouter {
    pub fn new(bindings: Vec<CcBinding>) -> Self {
        let len = bindings.len();
        Self {
            bindings,
            state: vec![RuntimeState::default(); len],
        }
    }

    pub fn route(&mut self, channel: u8, cc: u8, value: u8) -> Option<InputParameters> {
        for i in 0..self.bindings.len() {
            match self.bindings[i] {
                CcBinding::Float {
                    channel: c,
                    cc: n,
                    target,
                    curve,
                } if c == channel && n == cc => {
                    return Some(target.into_param(curve.apply(value)));
                }
                CcBinding::Bool {
                    channel: c,
                    cc: n,
                    target,
                    mode,
                } if c == channel && n == cc => {
                    let st = &mut self.state[i];
                    return match mode {
                        BoolMode::Latched => Some(target.into_param(value >= 64)),
                        BoolMode::Toggle { threshold } => {
                            let now_high = value >= threshold;
                            let was_high = st.last_cc_value >= threshold;
                            st.last_cc_value = value;
                            if now_high && !was_high {
                                st.toggle_state = !st.toggle_state;
                                Some(target.into_param(st.toggle_state))
                            } else {
                                None
                            }
                        }
                    };
                }
                _ => continue,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::super::mapping::{BoolTarget, FloatTarget};
    use super::*;
    use crate::dsp::mixer::{MixerBoolId, MixerFloatId, MixerParam};
    use crate::dsp::param::FloatCurve;

    fn float_binding(cc: u8, target: FloatTarget, curve: FloatCurve) -> CcBinding {
        CcBinding::Float { channel: 0, cc, target, curve }
    }

    fn bool_binding(cc: u8, target: BoolTarget, mode: BoolMode) -> CcBinding {
        CcBinding::Bool { channel: 0, cc, target, mode }
    }

    #[test]
    fn linear_curve_maps_full_range() {
        let mut r = CcRouter::new(vec![float_binding(
            1,
            FloatTarget::Mixer(MixerFloatId::MasterGainDb),
            FloatCurve::Linear { min: -60.0, max: 12.0 },
        )]);
        let p = r.route(0, 1, 127).unwrap();
        match p {
            InputParameters::Mixer(MixerParam::MasterGainDb(v)) => {
                assert!((v - 12.0).abs() < 1e-3, "got {v}");
            }
            _ => panic!("wrong variant: {p:?}"),
        }
    }

    #[test]
    fn unmatched_cc_returns_none() {
        let mut r = CcRouter::new(vec![float_binding(
            1,
            FloatTarget::Mixer(MixerFloatId::MasterGainDb),
            FloatCurve::Linear { min: -60.0, max: 12.0 },
        )]);
        assert!(r.route(0, 2, 64).is_none());
        assert!(r.route(1, 1, 64).is_none()); // wrong channel
    }

    #[test]
    fn latched_bool_threshold_at_64() {
        let mut r = CcRouter::new(vec![bool_binding(
            1,
            BoolTarget::Mixer(MixerBoolId::ReverbReturnMute),
            BoolMode::Latched,
        )]);
        match r.route(0, 1, 0) {
            Some(InputParameters::Mixer(MixerParam::ReverbReturnMute(false))) => {}
            other => panic!("expected mute=false, got {other:?}"),
        }
        match r.route(0, 1, 64) {
            Some(InputParameters::Mixer(MixerParam::ReverbReturnMute(true))) => {}
            other => panic!("expected mute=true, got {other:?}"),
        }
    }

    #[test]
    fn toggle_only_fires_on_rising_edge() {
        let mut r = CcRouter::new(vec![bool_binding(
            1,
            BoolTarget::Mixer(MixerBoolId::ReverbReturnMute),
            BoolMode::Toggle { threshold: 64 },
        )]);

        // First press: rising edge → toggle to true.
        match r.route(0, 1, 127) {
            Some(InputParameters::Mixer(MixerParam::ReverbReturnMute(true))) => {}
            other => panic!("expected toggle true, got {other:?}"),
        }
        // Hold high — no edge, no event.
        assert!(r.route(0, 1, 127).is_none());
        // Release — falling edge, no event.
        assert!(r.route(0, 1, 0).is_none());
        // Second press: rising edge → toggle back to false.
        match r.route(0, 1, 127) {
            Some(InputParameters::Mixer(MixerParam::ReverbReturnMute(false))) => {}
            other => panic!("expected toggle false, got {other:?}"),
        }
    }
}
