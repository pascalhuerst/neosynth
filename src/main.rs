mod audio;

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const SAMPLE_RATE: u32 = 48_000;
const CAPTURE_CPU: usize = 2;
const PLAYBACK_CPU: usize = 3;

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

    let engine = audio::Engine::new("default".to_string(), "default".to_string(), SAMPLE_RATE);

    let (capture_handle, playback_handle) = engine.run(running, CAPTURE_CPU, PLAYBACK_CPU)?;

    capture_handle
        .join()
        .map_err(|_| anyhow::anyhow!("capture thread panicked"))?;
    playback_handle
        .join()
        .map_err(|_| anyhow::anyhow!("playback thread panicked"))?;

    tracing::info!("Shutdown complete");
    Ok(())
}
