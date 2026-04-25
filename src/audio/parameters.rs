use crate::dsp::reverb::ReverbParam;

#[derive(Debug, Clone, Copy)]
pub enum InputParameters {
    LinearGain(f64),
    Reverb(ReverbParam),
}
