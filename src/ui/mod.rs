use crate::audio::{InputParameterRingBufferProducer, InputParameters};

use anyhow::Result;
use ringbuf::traits::Producer;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

slint::include_modules!();

pub fn run(mut producer: InputParameterRingBufferProducer, running: Arc<AtomicBool>) -> Result<()> {
    let ui = MainWindow::new()?;

    ui.on_gain_changed(move |gain| {
        if producer
            .try_push(InputParameters::LinearGain(gain as f64))
            .is_err()
        {
            tracing::warn!("Parameter channel full, dropping gain update");
        }
    });

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
