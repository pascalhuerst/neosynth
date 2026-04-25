#![allow(dead_code)]

pub mod constants {
    pub const PI: f32 = std::f32::consts::PI;
    pub const TWO_PI: f32 = std::f32::consts::TAU;
    pub const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
    pub const SQRT_TWO: f32 = std::f32::consts::SQRT_2;

    pub const DNC_CONST: f32 = 1e-18;
}

pub mod crossfades {
    #[inline]
    pub fn crossfade(s1: f32, s2: f32, g1: f32, g2: f32) -> f32 {
        s1 * g1 + s2 * g2
    }

    #[inline]
    pub fn bipolar_crossfade(s1: f32, s2: f32, mix: f32) -> f32 {
        (1.0 - mix.abs()) * s1 + mix * s2
    }

    #[inline]
    pub fn unipolar_crossfade(s1: f32, s2: f32, mix: f32) -> f32 {
        (1.0 - mix) * s1 + mix * s2
    }
}

pub mod math {
    #[inline]
    pub fn sin(x: f32) -> f32 {
        let x2 = x * x;
        (((((x2 * -2.39e-8 + 2.7526e-6) * x2 - 0.000198409) * x2 + 0.00833333) * x2 - 0.166667)
            * x2
            + 1.0)
            * x
    }

    #[inline]
    pub fn cos(x: f32) -> f32 {
        let x2 = x * x;
        ((((x2 * -2.605e-7 + 2.47609e-5) * x2 - 0.00138884) * x2 + 0.0416666) * x2 - 0.499923) * x2
            + 1.0
    }

    #[inline]
    pub fn tan(x: f32) -> f32 {
        let x2 = x * x;
        let x3 = x2 * x;
        let x5 = x3 * x2;
        0.133333 * x5 + 0.333333 * x3 + x
    }

    #[inline]
    fn arctan_poly(x: f32) -> f32 {
        let xs = x * x;
        ((((((((xs * 0.00286623 - 0.0161857) * xs + 0.0429096) * xs - 0.0752896) * xs
            + 0.106563)
            * xs
            - 0.142089)
            * xs
            + 0.199936)
            * xs
            - 0.333331)
            * xs
            + 1.0)
            * x
    }

    #[inline]
    pub fn arctan(x: f32) -> f32 {
        if x > 1.0 {
            1.5708 - arctan_poly(1.0 / x)
        } else if x < -1.0 {
            -1.5708 - arctan_poly(1.0 / x)
        } else {
            arctan_poly(x)
        }
    }

    #[inline]
    pub fn sin_p3_wrap(mut x: f32) -> f32 {
        x -= 0.25;
        if x >= 0.0 {
            x -= (x + 0.5) as i32 as f32;
        } else {
            x -= (x - 0.5) as i32 as f32;
        }
        x += x;
        x = x.abs();
        x = 0.5 - x;
        let x2 = x * x;
        x * ((2.26548 * x2 - 5.13274) * x2 + 3.14159)
    }

    #[inline]
    pub fn sin_p3_no_wrap(mut x: f32) -> f32 {
        x += x;
        x = x.abs();
        x = 0.5 - x;
        let x2 = x * x;
        x * ((2.26548 * x2 - 5.13274) * x2 + 3.14159)
    }

    #[inline]
    pub fn interpol_rt(fract: f32, sm1: f32, s0: f32, sp1: f32, sp2: f32) -> f32 {
        let f2 = fract * fract;
        let f3 = f2 * fract;
        let a = 0.5 * (sp1 - sm1);
        let b = 0.5 * (sp2 - s0);
        let c = s0 - sp1;
        s0 + fract * a + f3 * (a + b + 2.0 * c) - f2 * (2.0 * a + b + 3.0 * c)
    }

    #[inline]
    pub fn bell(x: f32) -> f32 {
        let x = (x - 0.5).abs() * 4.0 - 1.0;
        (2.0 - x.abs()) * x * -0.5 + 0.5
    }
}

pub mod conversion {
    #[inline]
    pub fn db_to_amp(db: f32) -> f32 {
        1.12202_f32.powf(db)
    }

    #[inline]
    pub fn amp_to_db(amp: f32) -> f32 {
        let amp = if amp == 0.0 { 1e15 } else { amp };
        20.0 * amp.log10()
    }

    #[inline]
    pub fn pitch_to_freq(pitch: f32) -> f32 {
        2.0_f32.powf((pitch - 69.0) / 12.0) * 440.0
    }

    #[inline]
    pub fn float_to_int(value: f32) -> i32 {
        if value >= 0.0 {
            (value + 0.5) as i32
        } else {
            (value - 0.5) as i32
        }
    }
}

pub mod others {
    #[inline]
    pub fn three_ranges(sample: f32, ctrl: f32, fold: f32) -> f32 {
        if ctrl < -0.25 {
            (sample + 1.0) * fold - 1.0
        } else if ctrl > 0.25 {
            (sample - 1.0) * fold + 1.0
        } else {
            sample
        }
    }

    #[inline]
    pub fn par_asym(sample: f32, sample_squared: f32, asym: f32) -> f32 {
        (1.0 - asym) * sample + 2.0 * asym * sample_squared
    }
}

pub mod curves {
    #[inline]
    pub fn apply_sine_curve(input: f32) -> f32 {
        let x = input * 2.0 - 1.0;
        x * x * x * -0.25 + x * 0.75 + 0.5
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Shaper1Bp {
        factor1: f32,
        factor2: f32,
        startpoint: f32,
        breakpoint: f32,
        endpoint: f32,
    }

    impl Shaper1Bp {
        const SPLIT: f32 = 0.5;

        pub fn new(startpoint: f32, breakpoint: f32, endpoint: f32) -> Self {
            let mut s = Self {
                factor1: 0.0,
                factor2: 0.0,
                startpoint: 0.0,
                breakpoint: 0.0,
                endpoint: 0.0,
            };
            s.set_curve(startpoint, breakpoint, endpoint);
            s
        }

        pub fn set_curve(&mut self, startpoint: f32, breakpoint: f32, endpoint: f32) {
            self.factor1 = (breakpoint - startpoint) / Self::SPLIT;
            self.factor2 = (endpoint - breakpoint) / (1.0 - Self::SPLIT);
            self.startpoint = startpoint;
            self.breakpoint = breakpoint;
            self.endpoint = endpoint;
        }

        #[inline]
        pub fn apply(&self, input: f32) -> f32 {
            if input <= Self::SPLIT {
                input * self.factor1 + self.startpoint
            } else {
                (input - Self::SPLIT) * self.factor2 + self.breakpoint
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct Shaper2Bp {
        factor1: f32,
        factor2: f32,
        factor3: f32,
        startpoint: f32,
        breakpoint1: f32,
        breakpoint2: f32,
        endpoint: f32,
    }

    impl Shaper2Bp {
        const SPLIT1: f32 = 1.0 / 3.0;
        const SPLIT2: f32 = 2.0 / 3.0;

        pub fn new(startpoint: f32, breakpoint1: f32, breakpoint2: f32, endpoint: f32) -> Self {
            let mut s = Self {
                factor1: 0.0,
                factor2: 0.0,
                factor3: 0.0,
                startpoint: 0.0,
                breakpoint1: 0.0,
                breakpoint2: 0.0,
                endpoint: 0.0,
            };
            s.set_curve(startpoint, breakpoint1, breakpoint2, endpoint);
            s
        }

        pub fn set_curve(
            &mut self,
            startpoint: f32,
            breakpoint1: f32,
            breakpoint2: f32,
            endpoint: f32,
        ) {
            self.factor1 = (breakpoint1 - startpoint) / Self::SPLIT1;
            self.factor2 = (breakpoint2 - breakpoint1) / (Self::SPLIT2 - Self::SPLIT1);
            self.factor3 = (endpoint - breakpoint2) / (1.0 - Self::SPLIT2);
            self.startpoint = startpoint;
            self.breakpoint1 = breakpoint1;
            self.breakpoint2 = breakpoint2;
            self.endpoint = endpoint;
        }

        #[inline]
        pub fn apply(&self, input: f32) -> f32 {
            if input <= Self::SPLIT1 {
                input * self.factor1 + self.startpoint
            } else if input <= Self::SPLIT2 {
                (input - Self::SPLIT1) * self.factor2 + self.breakpoint1
            } else {
                (input - Self::SPLIT2) * self.factor3 + self.breakpoint2
            }
        }
    }

    #[inline]
    pub fn squared_curvature(value: f32, curvature: f32) -> f32 {
        value * (1.0 + curvature * (value.abs() - 1.0))
    }
}
