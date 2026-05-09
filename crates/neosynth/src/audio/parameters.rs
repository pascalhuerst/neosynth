use crate::dsp::compressor::CompressorParam;
use crate::dsp::echo::EchoParam;
use crate::dsp::mixer::MixerParam;
use crate::dsp::reverb::ReverbParam;

#[derive(Debug, Clone, Copy)]
pub enum InputParameters {
    Reverb(ReverbParam),
    Echo(EchoParam),
    Mixer(MixerParam),
    Compressor(CompressorParam),
}
