use super::channels::{
    InputParameterRingBufferConsumer, SamplesRingBufferConsumer, SamplesRingBufferProducer,
};
use super::meters::MetersOutput;
use super::parameters::InputParameters;
use super::realtime::{prioritize_thread, set_thread_affinity};
use crate::dsp::compressor::Compressor;
use crate::dsp::mixer::Mixer;
use crate::dsp::reverb::Reverb;
use crate::dsp::stereo_delay::StereoDelay;
use crate::dsp::tape_delay::TapeDelay;

use ringbuf::traits::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

const NUM_OUTPUT_CHANNELS: usize = 2;

/// Spawn the DSP worker thread.
///
/// The worker runs the entire mixer + reverb + stereo_delay chain. It owns the
/// parameter consumers (UI/MIDI/OSC) so live param edits land here without
/// crossing the callback thread. Communication with the callback thread is
/// two SPSC f32 ringbufs: one carrying deinterleaved input samples, one
/// carrying deinterleaved output samples.
///
/// The worker busy-waits (spin loop) on input availability instead of
/// sleeping — it lives on its own isolated core, so a yield would only cost
/// us latency.
pub fn start_worker_thread(
    sample_rate: u32,
    period_size: usize,
    num_input_channels: usize,
    mut audio_in: SamplesRingBufferConsumer,
    mut audio_out: SamplesRingBufferProducer,
    mut ui_params: InputParameterRingBufferConsumer,
    mut midi_params: InputParameterRingBufferConsumer,
    mut osc_params: InputParameterRingBufferConsumer,
    meters: Arc<MetersOutput>,
    worker_cpu: usize,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        set_thread_affinity(worker_cpu);
        prioritize_thread();

        let input_total = period_size * num_input_channels;
        let output_total = period_size * NUM_OUTPUT_CHANNELS;

        let mut input_float = vec![0.0f32; input_total];
        let mut output_float = vec![0.0f32; output_total];
        let mut frame: Vec<f32> = vec![0.0; num_input_channels];

        let mut reverb = Reverb::new(sample_rate as f32, 1);
        let mut stereo_delay = StereoDelay::new(sample_rate as f32, 1);
        let mut tape_delay = TapeDelay::new(sample_rate as f32);
        let mut mixer = Mixer::new(sample_rate as f32, num_input_channels);
        let mut comp = Compressor::new(sample_rate as f32);

        tracing::info!(
            "Worker thread loop running, period_size={}, in_ch={}, out_ch={}",
            period_size,
            num_input_channels,
            NUM_OUTPUT_CHANNELS,
        );

        while running.load(Ordering::Relaxed) {
            // Drain parameter updates first so any in-flight edits are applied
            // before the period that's about to be processed.
            while let Some(update) = ui_params.try_pop() {
                dispatch(update, &mut reverb, &mut stereo_delay, &mut tape_delay, &mut mixer, &mut comp);
            }
            while let Some(update) = midi_params.try_pop() {
                dispatch(update, &mut reverb, &mut stereo_delay, &mut tape_delay, &mut mixer, &mut comp);
            }
            while let Some(update) = osc_params.try_pop() {
                dispatch(update, &mut reverb, &mut stereo_delay, &mut tape_delay, &mut mixer, &mut comp);
            }

            // Spin until a full period of input is available. Cheap atomic
            // load + spin_loop hint — no syscall, no context switch.
            while audio_in.occupied_len() < input_total {
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                std::hint::spin_loop();
            }

            let popped = audio_in.pop_slice(&mut input_float);
            debug_assert_eq!(popped, input_total);

            mixer.reset_levels();
            let mut comp_peak_gr_db: f32 = 0.0;

            for i in 0..period_size {
                for ch in 0..num_input_channels {
                    frame[ch] = input_float[ch * period_size + i];
                }

                mixer.process_inputs(&frame);
                reverb.apply(mixer.reverb_bus_l, mixer.reverb_bus_r);
                stereo_delay.apply(mixer.stereo_delay_bus_l, mixer.stereo_delay_bus_r);
                tape_delay.apply(mixer.tape_delay_bus_l, mixer.tape_delay_bus_r);
                mixer.add_returns(
                    reverb.out_l,
                    reverb.out_r,
                    stereo_delay.out_l,
                    stereo_delay.out_r,
                    tape_delay.out_l,
                    tape_delay.out_r,
                );

                // Pre-fader compressor: operates on the summed master before
                // the fader, so threshold settings stay invariant regardless
                // of master gain. The fader (and HPF + master metering) is
                // applied by `finalize()` to the compressed signal.
                comp.apply(mixer.master_l, mixer.master_r);
                if comp.gr_db > comp_peak_gr_db {
                    comp_peak_gr_db = comp.gr_db;
                }
                mixer.master_l = comp.out_l;
                mixer.master_r = comp.out_r;

                mixer.finalize();

                output_float[i] = mixer.master_l;
                output_float[period_size + i] = mixer.master_r;
            }

            // Publish per-bus levels (lock-free, latest-value-wins).
            let l = mixer.levels();
            let n = period_size as f32;
            let two_n = 2.0 * n;
            for (idx, (&peak, &sum_sq)) in
                l.input_peaks.iter().zip(l.input_sum_sq.iter()).enumerate()
            {
                meters.store_input(idx, peak, (sum_sq / n).sqrt());
            }
            meters.store_reverb(l.reverb_peak, (l.reverb_sum_sq / two_n).sqrt());
            meters.store_stereo_delay(l.stereo_delay_peak, (l.stereo_delay_sum_sq / two_n).sqrt());
            meters.store_tape_delay(l.tape_delay_peak, (l.tape_delay_sum_sq / two_n).sqrt());
            meters.store_master(
                l.master_l_peak,
                (l.master_l_sum_sq / n).sqrt(),
                l.master_r_peak,
                (l.master_r_sum_sq / n).sqrt(),
            );
            meters.store_compressor_gr_db(comp_peak_gr_db);

            // Spin until there's room for the output. With a sensibly-sized
            // ringbuf and a callback thread that's keeping up, this almost
            // never spins.
            while audio_out.vacant_len() < output_total {
                if !running.load(Ordering::Relaxed) {
                    return;
                }
                std::hint::spin_loop();
            }
            let pushed = audio_out.push_slice(&output_float);
            debug_assert_eq!(pushed, output_total);
        }

        tracing::info!("Worker thread stopped");
    })
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn dispatch(
    update: InputParameters,
    reverb: &mut Reverb,
    stereo_delay: &mut StereoDelay,
    tape_delay: &mut TapeDelay,
    mixer: &mut Mixer,
    comp: &mut Compressor,
) {
    match update {
        InputParameters::Reverb(p) => reverb.update_param(p),
        InputParameters::StereoDelay(p) => stereo_delay.update_param(p),
        InputParameters::TapeDelay(p) => tape_delay.update_param(p),
        InputParameters::Mixer(p) => mixer.update_param(p),
        InputParameters::Compressor(p) => comp.update_param(p),
    }
}
