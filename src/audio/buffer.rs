use num_traits::{NumCast, Zero};

#[derive(Clone, Copy, Debug)]
pub struct GenericAudioBuffer<T, const SAMPLES_PER_BUFFER: usize>
where
    T: Copy + Zero + NumCast,
{
    samples: [T; SAMPLES_PER_BUFFER],
    index: usize,
}

impl<T, const SAMPLES_PER_BUFFER: usize> Default for GenericAudioBuffer<T, SAMPLES_PER_BUFFER>
where
    T: Copy + Zero + NumCast,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const SAMPLES_PER_BUFFER: usize> GenericAudioBuffer<T, SAMPLES_PER_BUFFER>
where
    T: Copy + Zero + NumCast,
{
    pub fn new() -> Self {
        Self {
            samples: [T::zero(); SAMPLES_PER_BUFFER],
            index: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.index
    }

    pub fn is_empty(&self) -> bool {
        self.index == 0
    }

    pub fn push(&mut self, sample: T) {
        self.samples[self.index] = sample;
        self.index = (self.index + 1) % SAMPLES_PER_BUFFER;
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.index == 0 {
            None
        } else {
            self.index -= 1;
            Some(self.samples[self.index])
        }
    }

    pub fn clear(&mut self) {
        self.samples.fill(T::zero());
        self.index = 0;
    }

    pub const fn byte_size() -> usize {
        std::mem::size_of::<T>() * SAMPLES_PER_BUFFER
    }

    pub fn get_all(&self) -> &[T] {
        &self.samples
    }
}

#[inline(always)]
pub fn copy_from_slice(src: &[f32], dst: &mut [f32]) {
    dst.copy_from_slice(src);
}

#[inline(always)]
pub fn add_assign_elements(src: &[f32], dst: &mut [f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += *s;
    }
}

#[inline(always)]
pub fn multiply_scalar(src: &[f32], dst: &mut [f32], scalar: f32) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = *s * scalar;
    }
}

#[inline(always)]
pub fn multiply_scalar_in_place(buffer: &mut [f32], scalar: f32) {
    for sample in buffer.iter_mut() {
        *sample = *sample * scalar;
    }
}

#[inline(always)]
pub fn multiply_scalar_and_add(src: &[f32], dst: &mut [f32], scalar: f32) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += *s * scalar;
    }
}

#[inline(always)]
pub fn get_abs_peak(buffer: &[f32]) -> f32 {
    buffer.iter().map(|&x| x.abs()).fold(0., f32::max)
}

#[inline(always)]
pub fn zero_slice(buffer: &mut [f32]) {
    for sample in buffer.iter_mut() {
        *sample = 0.;
    }
}
