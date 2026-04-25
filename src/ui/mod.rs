use crate::audio::{InputParameterRingBufferProducer, InputParameters};
use crate::dsp::reverb::ReverbParam;

use anyhow::Result;
use ringbuf::traits::Producer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

slint::include_modules!();

pub fn run(producer: InputParameterRingBufferProducer, running: Arc<AtomicBool>) -> Result<()> {
    let ui = MainWindow::new()?;
    let producer = Rc::new(RefCell::new(producer));

    let make_handler = |variant: fn(f64) -> InputParameters| {
        let producer = producer.clone();
        move |v: f32| {
            if producer.borrow_mut().try_push(variant(v as f64)).is_err() {
                tracing::warn!("Parameter channel full, dropping update");
            }
        }
    };

    let make_reverb_handler = |variant: fn(f64) -> ReverbParam| {
        let producer = producer.clone();
        move |v: f32| {
            let msg = InputParameters::Reverb(variant(v as f64));
            if producer.borrow_mut().try_push(msg).is_err() {
                tracing::warn!("Parameter channel full, dropping update");
            }
        }
    };

    ui.on_gain_changed(make_handler(InputParameters::LinearGain));
    ui.on_reverb_size_changed(make_reverb_handler(ReverbParam::Size));
    ui.on_reverb_feedback_changed(make_reverb_handler(ReverbParam::Feedback));
    ui.on_reverb_balance_changed(make_reverb_handler(ReverbParam::Balance));
    ui.on_reverb_pre_delay_changed(make_reverb_handler(ReverbParam::PreDelayMs));
    ui.on_reverb_hpf_changed(make_reverb_handler(ReverbParam::HpfHz));
    ui.on_reverb_lpf_changed(make_reverb_handler(ReverbParam::LpfHz));
    ui.on_reverb_chorus_changed(make_reverb_handler(ReverbParam::Chorus));
    ui.on_reverb_dry_changed(make_reverb_handler(ReverbParam::Dry));
    ui.on_reverb_wet_changed(make_reverb_handler(ReverbParam::Wet));
    ui.on_reverb_send_changed(make_reverb_handler(ReverbParam::Send));

    let shutdown_timer = slint::Timer::default();
    {
        let running = running.clone();
        shutdown_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if !running.load(Ordering::Relaxed) {
                    let _ = slint::quit_event_loop();
                }
            },
        );
    }

    ui.run()?;

    running.store(false, Ordering::SeqCst);
    Ok(())
}
