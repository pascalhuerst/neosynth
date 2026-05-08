mod device;
pub mod mapping;
mod router;

use crate::audio::InputParameterRingBufferProducer;
use crate::persist::PersistableState;

use anyhow::Result;
use ringbuf::traits::Producer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use device::RawMidiInput;
use mapping::default_mapping;
use router::CcRouter;

/// Spawn the MIDI thread.
///
/// Opens an ALSA raw MIDI capture device by name (e.g. `"hw:1,0,0"`), reads
/// raw bytes through a streaming parser, translates Control Change events
/// through the default mapping into `InputParameters`, and pushes onto
/// `producer`. Each translated event is also fanned out into `persisted` so
/// the on-disk state mirrors MIDI-driven changes.
pub fn run(
    running: Arc<AtomicBool>,
    producer: InputParameterRingBufferProducer,
    persisted: Arc<Mutex<PersistableState>>,
    device_name: String,
    num_inputs: usize,
) -> Result<JoinHandle<()>> {
    let mut input = RawMidiInput::open(&device_name)?;
    let bindings = default_mapping(num_inputs);
    let mut router = CcRouter::new(bindings);
    let mut producer = producer;

    let handle = std::thread::spawn(move || {
        if let Err(e) = run_loop(&mut input, &mut router, &mut producer, &persisted, &running) {
            tracing::error!("MIDI thread error: {}", e);
        }
        tracing::info!("MIDI thread stopped");
    });
    Ok(handle)
}

fn run_loop(
    input: &mut RawMidiInput,
    router: &mut CcRouter,
    producer: &mut InputParameterRingBufferProducer,
    persisted: &Arc<Mutex<PersistableState>>,
    running: &AtomicBool,
) -> Result<()> {
    while running.load(Ordering::Relaxed) {
        input.poll_with_timeout(100)?;
        input.drain_cc(|channel, cc, value| {
            if let Some(param) = router.route(channel, cc, value) {
                if producer.try_push(param).is_err() {
                    tracing::warn!("MIDI parameter channel full, dropping update");
                }
                if let Ok(mut g) = persisted.lock() {
                    g.apply_and_mark_dirty(param);
                }
            }
        })?;
    }
    Ok(())
}
