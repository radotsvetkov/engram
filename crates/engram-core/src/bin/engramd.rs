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
//!   ENGRAM_HOME        brain state dir for the audit ledger (default "./brain")
//!   RUST_LOG           tracing filter (default "info")

use std::path::PathBuf;
use std::time::Duration;

use engram_core::{run_until_idle, Activity, Bus, Ledger, Priority, Spike, WakeReason, VERSION};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    init_tracing();

    let idle_after = Duration::from_secs(env_u64("ENGRAM_IDLE_SECS", 90));
    let home = PathBuf::from(std::env::var("ENGRAM_HOME").unwrap_or_else(|_| "./brain".into()));

    // Open the audit ledger first: every later change is recorded here, signed and
    // chained, before it touches anything else.
    let ledger = match Ledger::open(&home) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "failed to open audit ledger");
            std::process::exit(1);
        }
    };

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
    if let Ok(entry) = ledger.append("core.boot", "core", json!({ "version": VERSION })) {
        tracing::info!(seq = entry.seq, hash = %short(&entry.hash), "ledger: boot recorded");
    }
    bus.emit(Spike::new("core.boot", Priority::High, json!({ "version": VERSION })));

    let reason = run_until_idle(activity, idle_after).await;
    let why = match reason {
        WakeReason::Idle => "idle",
        WakeReason::Signal => "signal",
    };
    let _ = ledger.append("core.sleep", "core", json!({ "reason": why }));

    // Prove the chain is intact on the way out — auditability you can see.
    match ledger.verify() {
        Ok(n) => {
            let (seq, head) = ledger.head();
            tracing::info!(entries = n, head_seq = seq, head = %short(&head), "ledger verified — sleeping to zero");
        }
        Err(e) => tracing::error!(error = %e, "ledger verification FAILED"),
    }
}

fn short(hex: &str) -> &str {
    &hex[..hex.len().min(12)]
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).compact().init();
}
