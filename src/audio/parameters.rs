use crate::dsp::echo::EchoParam;
use crate::dsp::output_mixer::ChannelMixerParam;
use crate::dsp::reverb::ReverbParam;

#[derive(Debug, Clone, Copy)]
pub enum InputParameters {
    LinearGain(f64),
    Reverb(ReverbParam),
    Echo(EchoParam),
    Mixer(ChannelMixerParam),
}
