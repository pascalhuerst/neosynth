/// Realtime-safe monotonic time in microseconds.
///
/// `clock_gettime(CLOCK_MONOTONIC)` is served from the vDSO on modern Linux,
/// so it doesn't trap into the kernel and is safe from inside the audio
/// callback. JUCE uses the same pattern; the reference processor implementation
/// we're following ports it directly.
#[inline(always)]
pub fn get_ticks_in_microseconds() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    (ts.tv_sec as i64 * 1_000_000) + (ts.tv_nsec / 1_000)
}

/// Fraction of one period (`frames / sample_rate`) consumed since `start_us`,
/// expressed as 0.0 .. 1.0 (1.0 = exactly at the budget; >1.0 = over).
/// Multiply by 100 to get a percentage.
#[inline(always)]
pub fn cpu_usage(start_us: i64, frames: usize, sample_rate: usize) -> f32 {
    let end_us = get_ticks_in_microseconds();
    let elapsed_us = (end_us - start_us) as f32;
    let period_us = frames as f32 / sample_rate as f32 * 1_000_000.0;
    elapsed_us / period_us
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn ticks_advance() {
        let a = get_ticks_in_microseconds();
        thread::sleep(Duration::from_millis(1));
        let b = get_ticks_in_microseconds();
        assert!(b > a);
        assert!(b - a >= 1_000);
    }
}
