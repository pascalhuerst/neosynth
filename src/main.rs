mod audio;
mod dsp;
mod ui;

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SAMPLE_RATE: u32 = 48_000;
const AUDIO_CPU: usize = 2;
const PARAM_CHANNEL_CAPACITY: usize = 1024;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        ctrlc::set_handler(move || {
            tracing::info!("Ctrl+C received, shutting down");
            running.store(false, Ordering::SeqCst);
        })?;
    }

    let params = audio::create_parameter_channel(PARAM_CHANNEL_CAPACITY);

    let engine = audio::Engine::new("default".to_string(), "default".to_string(), SAMPLE_RATE);
    let audio_handle = engine.run(running.clone(), params.consumer, AUDIO_CPU)?;

    ui::run(params.producer, running.clone())?;

    audio_handle
        .join()
        .map_err(|_| anyhow::anyhow!("audio thread panicked"))?;

    tracing::info!("Shutdown complete");
    Ok(())
}
