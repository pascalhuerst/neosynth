use clap::ValueEnum;

/// PCM sample formats supported on the wire. Internal DSP is always f32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum SampleFormat {
    /// 16-bit signed little-endian, 2 bytes/sample.
    #[value(name = "s16le")]
    S16Le,
    /// 24-bit signed little-endian packed in 3 bytes (LSB first).
    #[value(name = "s24_3le")]
    S24_3Le,
    /// 24-bit signed big-endian packed in 3 bytes (MSB first).
    #[value(name = "s24_3be")]
    S24_3Be,
    /// 32-bit signed little-endian, 4 bytes/sample.
    #[value(name = "s32le")]
    S32Le,
}

impl SampleFormat {
    pub fn alsa_format(self) -> alsa::pcm::Format {
        use alsa::pcm::Format;
        match self {
            Self::S16Le => Format::S16LE,
            Self::S24_3Le => Format::S243LE,
            Self::S24_3Be => Format::S243BE,
            Self::S32Le => Format::S32LE,
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        match self {
            Self::S16Le => 2,
            Self::S24_3Le | Self::S24_3Be => 3,
            Self::S32Le => 4,
        }
    }

    /// Decode interleaved PCM bytes into a deinterleaved float buffer.
    ///
    /// `bytes`: input, length = `period_size * num_channels * bytes_per_sample`.
    /// `out`:   output f32, length = `period_size * num_channels`,
    ///          layout = channel blocks of `period_size` samples each.
    pub fn decode_to_float(self, bytes: &[u8], out: &mut [f32], num_channels: usize) {
        let bps = self.bytes_per_sample();
        let frame_bytes = num_channels * bps;
        debug_assert!(num_channels > 0);
        debug_assert_eq!(bytes.len() % frame_bytes, 0);
        let period = bytes.len() / frame_bytes;
        debug_assert_eq!(out.len(), period * num_channels);
        match self {
            Self::S16Le => decode_s16le(bytes, out, period, num_channels),
            Self::S24_3Le => decode_s24_3le(bytes, out, period, num_channels),
            Self::S24_3Be => decode_s24_3be(bytes, out, period, num_channels),
            Self::S32Le => decode_s32le(bytes, out, period, num_channels),
        }
    }

    /// Inverse of `decode_to_float`. Clamps out-of-range samples to ±1.
    pub fn encode_from_float(self, src: &[f32], bytes: &mut [u8], num_channels: usize) {
        let bps = self.bytes_per_sample();
        let frame_bytes = num_channels * bps;
        debug_assert!(num_channels > 0);
        debug_assert_eq!(bytes.len() % frame_bytes, 0);
        let period = bytes.len() / frame_bytes;
        debug_assert_eq!(src.len(), period * num_channels);
        match self {
            Self::S16Le => encode_s16le(src, bytes, period, num_channels),
            Self::S24_3Le => encode_s24_3le(src, bytes, period, num_channels),
            Self::S24_3Be => encode_s24_3be(src, bytes, period, num_channels),
            Self::S32Le => encode_s32le(src, bytes, period, num_channels),
        }
    }
}

// --- Per-format scale factors and clamps ---

const S16_MAX: f32 = 32_767.0;
const S24_MAX: f32 = 8_388_607.0;
const S32_MAX: f32 = 2_147_483_647.0;

#[inline(always)]
fn clamp_unit(x: f32) -> f32 {
    x.clamp(-1.0, 1.0 - f32::EPSILON)
}

// --- S16LE ---

fn decode_s16le(bytes: &[u8], out: &mut [f32], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let i = (frame * channels + ch) * 2;
            let v = i16::from_le_bytes([bytes[i], bytes[i + 1]]);
            out[ch * period + frame] = v as f32 / S16_MAX;
        }
    }
}

fn encode_s16le(src: &[f32], bytes: &mut [u8], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let v = (clamp_unit(src[ch * period + frame]) * S16_MAX) as i16;
            let i = (frame * channels + ch) * 2;
            let b = v.to_le_bytes();
            bytes[i] = b[0];
            bytes[i + 1] = b[1];
        }
    }
}

// --- S24_3LE: 24-bit signed, 3 bytes per sample, LSB first ---

fn decode_s24_3le(bytes: &[u8], out: &mut [f32], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let i = (frame * channels + ch) * 3;
            let b0 = bytes[i];
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            // Sign-extend 24-bit by composing a 4-byte LE i32 with the high byte
            // filled from b2's MSB.
            let sign = if b2 & 0x80 != 0 { 0xFF } else { 0x00 };
            let v = i32::from_le_bytes([b0, b1, b2, sign]);
            out[ch * period + frame] = v as f32 / S24_MAX;
        }
    }
}

fn encode_s24_3le(src: &[f32], bytes: &mut [u8], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let v = (clamp_unit(src[ch * period + frame]) * S24_MAX) as i32;
            let b = v.to_le_bytes();
            let i = (frame * channels + ch) * 3;
            bytes[i] = b[0];
            bytes[i + 1] = b[1];
            bytes[i + 2] = b[2];
        }
    }
}

// --- S24_3BE: same value range, MSB first ---

fn decode_s24_3be(bytes: &[u8], out: &mut [f32], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let i = (frame * channels + ch) * 3;
            let b0 = bytes[i];
            let b1 = bytes[i + 1];
            let b2 = bytes[i + 2];
            let sign = if b0 & 0x80 != 0 { 0xFF } else { 0x00 };
            let v = i32::from_be_bytes([sign, b0, b1, b2]);
            out[ch * period + frame] = v as f32 / S24_MAX;
        }
    }
}

fn encode_s24_3be(src: &[f32], bytes: &mut [u8], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let v = (clamp_unit(src[ch * period + frame]) * S24_MAX) as i32;
            let b = v.to_be_bytes();
            // top byte of i32 is sign extension; use the lower three bytes
            let i = (frame * channels + ch) * 3;
            bytes[i] = b[1];
            bytes[i + 1] = b[2];
            bytes[i + 2] = b[3];
        }
    }
}

// --- S32LE ---

fn decode_s32le(bytes: &[u8], out: &mut [f32], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let i = (frame * channels + ch) * 4;
            let v = i32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
            out[ch * period + frame] = v as f32 / S32_MAX;
        }
    }
}

fn encode_s32le(src: &[f32], bytes: &mut [u8], period: usize, channels: usize) {
    for frame in 0..period {
        for ch in 0..channels {
            let v = (clamp_unit(src[ch * period + frame]) * S32_MAX) as i32;
            let b = v.to_le_bytes();
            let i = (frame * channels + ch) * 4;
            bytes[i] = b[0];
            bytes[i + 1] = b[1];
            bytes[i + 2] = b[2];
            bytes[i + 3] = b[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(fmt: SampleFormat) {
        let channels = 2;
        let period = 64;
        // Some interesting values across the dynamic range.
        let mut src = vec![0.0f32; period * channels];
        for ch in 0..channels {
            for f in 0..period {
                let phase = (f as f32) * 0.1 + (ch as f32) * 0.05;
                src[ch * period + f] = phase.sin() * 0.7;
            }
        }
        let mut bytes = vec![0u8; period * channels * fmt.bytes_per_sample()];
        fmt.encode_from_float(&src, &mut bytes, channels);

        let mut decoded = vec![0.0f32; period * channels];
        fmt.decode_to_float(&bytes, &mut decoded, channels);

        // Quantization tolerance per format: ~1 LSB.
        let tol = match fmt {
            SampleFormat::S16Le => 1.0 / S16_MAX,
            SampleFormat::S24_3Le | SampleFormat::S24_3Be => 1.0 / S24_MAX,
            SampleFormat::S32Le => 1.0 / S32_MAX,
        } * 2.0;

        for i in 0..src.len() {
            assert!(
                (src[i] - decoded[i]).abs() < tol,
                "fmt {:?} mismatch at {}: src={} dec={} tol={}",
                fmt,
                i,
                src[i],
                decoded[i],
                tol
            );
        }
    }

    #[test]
    fn s16le_round_trip() {
        round_trip(SampleFormat::S16Le);
    }

    #[test]
    fn s24_3le_round_trip() {
        round_trip(SampleFormat::S24_3Le);
    }

    #[test]
    fn s24_3be_round_trip() {
        round_trip(SampleFormat::S24_3Be);
    }

    #[test]
    fn s32le_round_trip() {
        round_trip(SampleFormat::S32Le);
    }

    #[test]
    fn s24_3le_endianness_sanity() {
        // -1 in 24-bit signed = 0xFFFFFF
        // -1 / S24_MAX ≈ -1.19e-7 (one LSB below zero)
        let bytes = [0xFFu8, 0xFF, 0xFF, /* second sample */ 0x00, 0x00, 0x80];
        let mut out = [0.0f32; 2];
        SampleFormat::S24_3Le.decode_to_float(&bytes, &mut out, 1);
        assert!(out[0] < 0.0 && out[0] > -1e-6);
        // 0x800000 = -8388608 = the most negative 24-bit value = -1.0
        assert!((out[1] - (-1.0)).abs() < 1e-3, "got {}", out[1]);
    }

    #[test]
    fn s24_3be_endianness_sanity() {
        // Most negative 24-bit value, big-endian: [0x80, 0x00, 0x00]
        let bytes = [0x80u8, 0x00, 0x00];
        let mut out = [0.0f32; 1];
        SampleFormat::S24_3Be.decode_to_float(&bytes, &mut out, 1);
        assert!((out[0] - (-1.0)).abs() < 1e-3, "got {}", out[0]);
    }

    #[test]
    fn clamp_handles_overshoot() {
        // 1.5 should clamp to nearly 1.0
        let src = [1.5f32, -1.5];
        let mut bytes = [0u8; 8];
        SampleFormat::S32Le.encode_from_float(&src, &mut bytes, 1);
        let mut decoded = [0.0f32; 2];
        SampleFormat::S32Le.decode_to_float(&bytes, &mut decoded, 1);
        assert!((decoded[0] - (1.0 - f32::EPSILON)).abs() < 1e-6);
        assert!((decoded[1] - (-1.0)).abs() < 1e-6);
    }
}
