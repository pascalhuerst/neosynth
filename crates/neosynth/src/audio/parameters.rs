use crate::dsp::compressor::CompressorParam;
use crate::dsp::mixer::MixerParam;
use crate::dsp::reverb::ReverbParam;
use crate::dsp::stereo_delay::StereoDelayParam;
use crate::dsp::tape_delay::TapeDelayParam;

#[derive(Debug, Clone, Copy)]
pub enum InputParameters {
    Reverb(ReverbParam),
    StereoDelay(StereoDelayParam),
    TapeDelay(TapeDelayParam),
    Mixer(MixerParam),
    Compressor(CompressorParam),
}
