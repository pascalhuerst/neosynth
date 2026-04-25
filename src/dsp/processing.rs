#[inline]
pub fn process_linear_gain(buffer: &mut [f32], gain: f32) {
    for sample in buffer.iter_mut() {
        *sample *= gain;
    }
}
