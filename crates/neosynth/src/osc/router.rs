use std::collections::HashMap;

use rosc::OscType;

use crate::audio::InputParameters;
use crate::dsp::compressor::CompressorParamKind;
use crate::dsp::mixer::{self, MixerBoolId, MixerFloatId, MixerParamId};
use crate::dsp::param::FloatParams;
use crate::dsp::reverb::ReverbParamKind;
use crate::dsp::stereo_delay::StereoDelayParamKind;
use crate::dsp::tape_delay::TapeDelayParamKind;
use crate::persist::AppState;

/// Current value of an OSC-addressable parameter, returned by
/// `OscRouter::current_value`. Used by the `/get_all` reply.
#[derive(Debug, Clone, Copy)]
pub enum OscValue {
    Float(f32),
    Bool(bool),
}

/// One known OSC-addressable parameter (after path expansion).
#[derive(Debug, Clone)]
pub struct OscParameter {
    pub path: String,
    pub kind: OscParamKind,
}

#[derive(Debug, Clone, Copy)]
pub enum OscParamKind {
    Float { min: f32, max: f32, default: f32 },
    Bool { default: bool },
}

/// Internal route entry — what to do when this path is hit by an inbound message.
#[derive(Clone, Copy)]
enum RouteTarget {
    Reverb(ReverbParamKind),
    StereoDelay(StereoDelayParamKind),
    TapeDelay(TapeDelayParamKind),
    Compressor(CompressorParamKind),
    MixerFloat(MixerFloatId),
    MixerBool(MixerBoolId),
}

/// Translates incoming OSC messages into `InputParameters` events.
/// Built once at startup from the trait-derived parameter list.
pub struct OscRouter {
    routes: HashMap<String, RouteTarget>,
    /// Cached introspection list, in display order, for `/list` queries.
    introspection: Vec<OscParameter>,
}

impl OscRouter {
    pub fn new(num_inputs: usize) -> Self {
        let mut routes: HashMap<String, RouteTarget> = HashMap::new();
        let mut introspection: Vec<OscParameter> = Vec::new();

        // Effect float params (auto-derived via FloatParams trait)
        push_effect::<ReverbParamKind, _>(&mut routes, &mut introspection, RouteTarget::Reverb);
        push_effect::<StereoDelayParamKind, _>(&mut routes, &mut introspection, RouteTarget::StereoDelay);
        push_effect::<TapeDelayParamKind, _>(&mut routes, &mut introspection, RouteTarget::TapeDelay);
        push_effect::<CompressorParamKind, _>(
            &mut routes,
            &mut introspection,
            RouteTarget::Compressor,
        );

        // Mixer params — special-cased because they use indexed paths and split float/bool
        for id in mixer::default_param_order(num_inputs) {
            match id {
                MixerParamId::Float(f) => {
                    let path = f.osc_path();
                    let (min, max) = f.default_curve().range();
                    introspection.push(OscParameter {
                        path: path.clone(),
                        kind: OscParamKind::Float {
                            min,
                            max,
                            default: f.default_value(),
                        },
                    });
                    routes.insert(path, RouteTarget::MixerFloat(f));
                }
                MixerParamId::Bool(b) => {
                    let path = b.osc_path();
                    introspection.push(OscParameter {
                        path: path.clone(),
                        kind: OscParamKind::Bool {
                            default: b.default_value(),
                        },
                    });
                    routes.insert(path, RouteTarget::MixerBool(b));
                }
            }
        }

        Self { routes, introspection }
    }

    /// Translate an inbound OSC message to an `InputParameters` event.
    /// Returns `None` if the path is unknown or the args don't match.
    pub fn route(&self, addr: &str, args: &[OscType]) -> Option<InputParameters> {
        let target = self.routes.get(addr)?;
        match target {
            RouteTarget::Reverb(k) => {
                let v = first_float(args)?;
                Some(InputParameters::Reverb(k.build(v as f64)))
            }
            RouteTarget::StereoDelay(k) => {
                let v = first_float(args)?;
                Some(InputParameters::StereoDelay(k.build(v as f64)))
            }
            RouteTarget::TapeDelay(k) => {
                let v = first_float(args)?;
                Some(InputParameters::TapeDelay(k.build(v as f64)))
            }
            RouteTarget::Compressor(k) => {
                let v = first_float(args)?;
                Some(InputParameters::Compressor(k.build(v as f64)))
            }
            RouteTarget::MixerFloat(f) => {
                let v = first_float(args)?;
                Some(InputParameters::Mixer(f.build(v as f64)))
            }
            RouteTarget::MixerBool(b) => {
                let v = first_bool(args)?;
                Some(InputParameters::Mixer(b.build(v)))
            }
        }
    }

    pub fn introspection(&self) -> &[OscParameter] {
        &self.introspection
    }

    /// Look up a parameter's current value out of `state`. Used to answer the
    /// `/get_all` query so a freshly-connected remote can sync.
    pub fn current_value(&self, addr: &str, state: &AppState) -> Option<OscValue> {
        let target = self.routes.get(addr)?;
        match target {
            RouteTarget::Reverb(k) => Some(OscValue::Float(k.read(&state.reverb) as f32)),
            RouteTarget::StereoDelay(k) => Some(OscValue::Float(k.read(&state.stereo_delay) as f32)),
            RouteTarget::TapeDelay(k) => Some(OscValue::Float(k.read(&state.tape_delay) as f32)),
            RouteTarget::Compressor(k) => Some(OscValue::Float(k.read(&state.compressor) as f32)),
            RouteTarget::MixerFloat(f) => f.read(&state.mixer).map(OscValue::Float),
            RouteTarget::MixerBool(b) => b.read(&state.mixer).map(OscValue::Bool),
        }
    }
}

fn push_effect<K: FloatParams, F: Fn(K) -> RouteTarget>(
    routes: &mut HashMap<String, RouteTarget>,
    introspection: &mut Vec<OscParameter>,
    wrap: F,
) {
    for &k in K::all() {
        let path = k.osc_path();
        let (min, max) = k.default_curve().range();
        introspection.push(OscParameter {
            path: path.clone(),
            kind: OscParamKind::Float {
                min,
                max,
                default: k.default_value() as f32,
            },
        });
        routes.insert(path, wrap(k));
    }
}

/// Accept the first numeric arg as f32 (Float, Int, Double all coerce).
fn first_float(args: &[OscType]) -> Option<f32> {
    args.first().and_then(|a| match a {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        OscType::Long(l) => Some(*l as f32),
        _ => None,
    })
}

/// Accept the first arg as bool (Bool, Int≠0, Float≥0.5, "T"/"F" symbols).
fn first_bool(args: &[OscType]) -> Option<bool> {
    args.first().and_then(|a| match a {
        OscType::Bool(b) => Some(*b),
        OscType::Int(i) => Some(*i != 0),
        OscType::Long(l) => Some(*l != 0),
        OscType::Float(f) => Some(*f >= 0.5),
        OscType::Double(d) => Some(*d >= 0.5),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::mixer::MixerParam;
    use crate::dsp::reverb::ReverbParam;

    #[test]
    fn reverb_path_routes_to_reverb_param() {
        let r = OscRouter::new(2);
        let p = r
            .route("/reverb/size", &[OscType::Float(0.7)])
            .expect("known path");
        match p {
            InputParameters::Reverb(ReverbParam::Size(v)) => {
                assert!((v - 0.7).abs() < 1e-6);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn mixer_indexed_input_path_routes() {
        let r = OscRouter::new(4);
        let p = r
            .route("/mixer/input/2/gain_db", &[OscType::Float(-6.0)])
            .expect("known path");
        match p {
            InputParameters::Mixer(MixerParam::InputGainDb(2, v)) => {
                assert!((v - -6.0).abs() < 1e-6);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn mute_path_takes_bool_or_int() {
        let r = OscRouter::new(2);
        match r
            .route("/mixer/input/0/mute", &[OscType::Bool(true)])
            .expect("known path")
        {
            InputParameters::Mixer(MixerParam::InputMute(0, true)) => {}
            other => panic!("wrong variant: {other:?}"),
        }
        match r
            .route("/mixer/input/0/mute", &[OscType::Int(1)])
            .expect("known path")
        {
            InputParameters::Mixer(MixerParam::InputMute(0, true)) => {}
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_path_returns_none() {
        let r = OscRouter::new(2);
        assert!(r.route("/nonsense", &[OscType::Float(1.0)]).is_none());
    }

    #[test]
    fn introspection_includes_all_effect_and_mixer_params() {
        let r = OscRouter::new(2);
        let paths: Vec<&str> = r.introspection().iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&"/reverb/size"));
        assert!(paths.contains(&"/stereo_delay/time_l_ms"));
        assert!(paths.contains(&"/mixer/input/0/gain_db"));
        assert!(paths.contains(&"/mixer/input/1/pan"));
        assert!(paths.contains(&"/mixer/master/gain_db"));
        assert!(paths.contains(&"/mixer/reverb_return/mute"));
    }
}
