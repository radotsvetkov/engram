//! Wake / sleep lifecycle.
//!
//! The core is awake only while something is happening. [`Activity`] records the
//! last moment of neural activity; [`run_until_idle`] resolves when the core has
//! been quiet for an idle window (so it can exit to zero RAM) or when the OS asks
//! it to stop. On a socket-activated VPS this means there is *no resident process*
//! between requests - the near-zero-idle property in one small module.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::{interval, Instant, MissedTickBehavior};

/// A shared, cheap-to-clone record of when the core was last active. Built on
/// Tokio's clock so it behaves correctly under paused-time tests.
#[derive(Clone)]
pub struct Activity {
    last: Arc<Mutex<Instant>>,
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

impl Activity {
    pub fn new() -> Self {
        Activity {
            last: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Record activity now - call this whenever a spike fires or a request lands.
    pub fn touch(&self) {
        *self.last.lock().expect("activity mutex poisoned") = Instant::now();
    }

    /// How long the core has been quiet.
    pub fn idle(&self) -> Duration {
        let last = *self.last.lock().expect("activity mutex poisoned");
        Instant::now().saturating_duration_since(last)
    }
}

/// Why the core is winding down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    /// The idle window elapsed with no activity - safe to sleep to zero.
    Idle,
    /// The OS asked us to stop (SIGINT/SIGTERM).
    Signal,
}

/// Poll the idle clock no more than every 5s and no less than every 10ms.
fn poll_interval(idle_after: Duration) -> Duration {
    (idle_after / 4).clamp(Duration::from_millis(10), Duration::from_secs(5))
}

/// Resolve once the core has been idle for `idle_after`.
pub async fn idle_watch(activity: &Activity, idle_after: Duration) {
    let mut tick = interval(poll_interval(idle_after));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if activity.idle() >= idle_after {
            return;
        }
    }
}

/// Run until the core is idle for `idle_after` or a shutdown signal arrives.
pub async fn run_until_idle(activity: Activity, idle_after: Duration) -> WakeReason {
    tokio::select! {
        _ = idle_watch(&activity, idle_after) => WakeReason::Idle,
        _ = shutdown_signal() => WakeReason::Signal,
    }
}

/// Resolve on the first SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_activity_is_not_idle() {
        let a = Activity::new();
        assert!(a.idle() < Duration::from_millis(500));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_watch_returns_after_window() {
        let a = Activity::new();
        // Under paused time, Tokio auto-advances the virtual clock while idle_watch
        // waits on its interval, so this completes promptly yet only after >= 90s
        // of virtual idle time.
        idle_watch(&a, Duration::from_secs(90)).await;
        assert!(a.idle() >= Duration::from_secs(90));
    }

    #[tokio::test(start_paused = true)]
    async fn touch_resets_idle() {
        let a = Activity::new();
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(a.idle() >= Duration::from_secs(60));
        a.touch();
        assert!(a.idle() < Duration::from_secs(1));
    }
}
