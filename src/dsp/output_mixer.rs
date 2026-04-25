use crate::dsp::dsp_toolbox::constants::TWO_PI;

#[derive(Debug, Clone, Copy)]
pub struct ChannelMixerParams {
    pub dry_mix: f32,
    pub reverb_mix: f32,
    pub level: f32,
}

impl Default for ChannelMixerParams {
    fn default() -> Self {
        Self {
            dry_mix: 1.0,
            reverb_mix: 0.3,
            level: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ChannelMixerParam {
    DryMix(f64),
    ReverbMix(f64),
    Level(f64),
}

pub struct ChannelMixer {
    pub out_l: f32,
    pub out_r: f32,

    params: ChannelMixerParams,

    hp_b0: f32,
    hp_state_l: f32,
    hp_state_r: f32,
}

impl ChannelMixer {
    pub fn new(sample_rate: f32) -> Self {
        let norm_omega = TWO_PI / sample_rate;
        Self {
            out_l: 0.0,
            out_r: 0.0,
            params: ChannelMixerParams::default(),
            hp_b0: norm_omega * 12.978,
            hp_state_l: 0.0,
            hp_state_r: 0.0,
        }
    }

    pub fn params(&self) -> &ChannelMixerParams {
        &self.params
    }

    pub fn set_params(&mut self, params: ChannelMixerParams) {
        self.params = params;
    }

    pub fn update_param(&mut self, param: ChannelMixerParam) {
        match param {
            ChannelMixerParam::DryMix(v) => self.params.dry_mix = v as f32,
            ChannelMixerParam::ReverbMix(v) => self.params.reverb_mix = v as f32,
            ChannelMixerParam::Level(v) => self.params.level = v as f32,
        }
    }

    pub fn reset(&mut self) {
        self.out_l = 0.0;
        self.out_r = 0.0;
        self.hp_state_l = 0.0;
        self.hp_state_r = 0.0;
    }

    #[inline]
    pub fn combine(&mut self, raw_l: f32, raw_r: f32, reverb_wet_l: f32, reverb_wet_r: f32) {
        let mix_l = raw_l * self.params.dry_mix + reverb_wet_l * self.params.reverb_mix;
        let mix_r = raw_r * self.params.dry_mix + reverb_wet_r * self.params.reverb_mix;

        let hp_l = mix_l - self.hp_state_l;
        self.hp_state_l = hp_l * self.hp_b0 + self.hp_state_l;
        let hp_r = mix_r - self.hp_state_r;
        self.hp_state_r = hp_r * self.hp_b0 + self.hp_state_r;

        self.out_l = hp_l * self.params.level;
        self.out_r = hp_r * self.params.level;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac_peak_l_after_settle(mixer: &mut ChannelMixer, dry_l: f32, wet_l: f32) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..10_000 {
            let sign = if i & 1 == 0 { 1.0 } else { -1.0 };
            mixer.combine(dry_l * sign, 0.0, wet_l * sign, 0.0);
            if i >= 1_000 {
                peak = peak.max(mixer.out_l.abs());
            }
        }
        peak
    }

    #[test]
    fn dry_only_passes_ac_input() {
        let mut mixer = ChannelMixer::new(48_000.0);
        mixer.set_params(ChannelMixerParams {
            dry_mix: 1.0,
            reverb_mix: 0.0,
            level: 1.0,
        });
        let peak = ac_peak_l_after_settle(&mut mixer, 0.5, 0.0);
        assert!((peak - 0.5).abs() < 1e-2, "peak: {peak}");
    }

    #[test]
    fn reverb_only_passes_wet_ac() {
        let mut mixer = ChannelMixer::new(48_000.0);
        mixer.set_params(ChannelMixerParams {
            dry_mix: 0.0,
            reverb_mix: 1.0,
            level: 1.0,
        });
        let peak = ac_peak_l_after_settle(&mut mixer, 0.0, 0.4);
        assert!((peak - 0.4).abs() < 1e-2, "peak: {peak}");
    }

    #[test]
    fn level_scales_output() {
        let mut mixer = ChannelMixer::new(48_000.0);
        mixer.set_params(ChannelMixerParams {
            dry_mix: 1.0,
            reverb_mix: 0.0,
            level: 0.5,
        });
        let peak = ac_peak_l_after_settle(&mut mixer, 1.0, 0.0);
        assert!((peak - 0.5).abs() < 1e-2, "peak: {peak}");
    }

    #[test]
    fn dc_blocker_kills_constant_offset() {
        let mut mixer = ChannelMixer::new(48_000.0);
        mixer.set_params(ChannelMixerParams::default());

        for _ in 0..480_000 {
            mixer.combine(0.5, 0.5, 0.0, 0.0);
        }
        assert!(
            mixer.out_l.abs() < 1e-3,
            "DC offset not blocked, out_l: {}",
            mixer.out_l
        );
    }
}
