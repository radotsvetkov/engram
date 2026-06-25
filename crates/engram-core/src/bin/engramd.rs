//! `engramd` — the Engram core daemon entrypoint.
//!
//! In `standalone` mode (the default, used for dev and simple VPS) it boots the
//! neural bus, attaches an observer, fires a boot spike, and then runs until it has
//! been idle long enough to sleep — demonstrating the full wake → activity → sleep
//! lifecycle. The `socket-activation` and `lambda` entrypoints (zero-idle prod and
//! serverless) build on the same kernel and arrive with the deploy phase.
//!
//! Tunables (env):
//!   ENGRAM_IDLE_SECS   idle window before sleeping to zero (default 90)
//!   RUST_LOG           tracing filter (default "info")

use std::time::Duration;

use engram_core::{run_until_idle, Activity, Bus, Priority, Spike, WakeReason, VERSION};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    init_tracing();

    let idle_after = Duration::from_secs(env_u64("ENGRAM_IDLE_SECS", 90));
    let bus = Bus::new(1024);
    let activity = Activity::new();

    // The cortex observer: a neuron that watches every spike, keeps the brain awake
    // while activity flows, and (later) streams the audit feed to the desktop.
    // Subscribe synchronously *before* emitting so the boot spike is not missed.
    {
        let mut syn = bus.synapse();
        let activity = activity.clone();
        tokio::spawn(async move {
            while let Some(spike) = syn.recv().await {
                activity.touch();
                tracing::info!(
                    id = spike.id,
                    topic = %spike.topic,
                    priority = ?spike.priority,
                    taint = ?spike.taint,
                    "spike"
                );
            }
        });
    }

    tracing::info!(version = VERSION, idle_after_s = idle_after.as_secs(), "engram core awake");
    bus.emit(Spike::new("core.boot", Priority::High, json!({ "version": VERSION })));

    let reason = run_until_idle(activity, idle_after).await;
    match reason {
        WakeReason::Idle => tracing::info!("idle window elapsed — sleeping to zero"),
        WakeReason::Signal => tracing::info!("signal received — sleeping to zero"),
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).compact().init();
}
