//! The neural substrate.
//!
//! A *spike* is the unit of activity - a neuron fires it, others react. Spikes flow
//! across four **priority lanes**, fastest first, so reflexes preempt deliberation
//! exactly as they do in a nervous system. The bus is in-process (Tokio broadcast
//! channels) and lives only while the core is awake; there is no parked daemon.
//!
//! Every spike carries a [`Taint`] tag. Anything derived from untrusted input (the
//! web, stored memory of unknown origin) is `Untrusted`, and taint is *monotonic* -
//! it only ever spreads. Downstream, an untrusted run is dropped to no-egress /
//! no-secrets, which is what breaks the prompt-injection → exfiltration chain.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Priority lanes, fastest first. A reflex preempts everything below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Hard-wired, must-run-now reactions (safety stops, cancellations).
    Reflex = 0,
    /// Time-sensitive work the user is waiting on.
    High = 1,
    /// The default lane for ordinary activity.
    Normal = 2,
    /// Background housekeeping (consolidation, metrics).
    Low = 3,
}

impl Priority {
    /// All lanes in priority order.
    pub const LANES: [Priority; 4] = [
        Priority::Reflex,
        Priority::High,
        Priority::Normal,
        Priority::Low,
    ];

    #[inline]
    fn index(self) -> usize {
        self as usize
    }
}

/// Provenance taint. `Untrusted` marks data influenced by an attacker-controllable
/// channel; it is sticky and only spreads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Taint {
    #[default]
    Trusted,
    Untrusted,
}

impl Taint {
    /// Combine taints. Trusted ∧ Trusted = Trusted; anything else is Untrusted.
    #[inline]
    pub fn join(self, other: Taint) -> Taint {
        match (self, other) {
            (Taint::Trusted, Taint::Trusted) => Taint::Trusted,
            _ => Taint::Untrusted,
        }
    }

    #[inline]
    pub fn is_untrusted(self) -> bool {
        matches!(self, Taint::Untrusted)
    }
}

/// One unit of neural activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spike {
    /// Process-local monotonic id, useful for ordering and audit references.
    pub id: u64,
    /// Dotted topic, e.g. `"core.boot"` or `"memory.write"`.
    pub topic: String,
    pub priority: Priority,
    pub taint: Taint,
    /// Unix epoch milliseconds when the spike was minted.
    pub ts_ms: u64,
    pub payload: serde_json::Value,
}

impl Spike {
    /// Mint a new trusted spike on the given topic and lane.
    pub fn new(topic: impl Into<String>, priority: Priority, payload: serde_json::Value) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(1);
        Spike {
            id: SEQ.fetch_add(1, Ordering::Relaxed),
            topic: topic.into(),
            priority,
            taint: Taint::Trusted,
            ts_ms: now_ms(),
            payload,
        }
    }

    /// Mark this spike (at least) as tainted by `taint`. Taint only spreads.
    pub fn tainted(mut self, taint: Taint) -> Self {
        self.taint = self.taint.join(taint);
        self
    }
}

/// Current wall-clock time in Unix epoch milliseconds (0 if the clock is before the epoch).
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The in-process event bus: one broadcast lane per [`Priority`]. Cheap to clone
/// (shares the same underlying senders), so producers and reactors hold a handle.
#[derive(Clone)]
pub struct Bus {
    lanes: Arc<[broadcast::Sender<Arc<Spike>>; 4]>,
    emitted: Arc<AtomicU64>,
}

impl Bus {
    /// Create a bus with `capacity` buffered spikes per lane (clamped to ≥ 1).
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        let lanes: [broadcast::Sender<Arc<Spike>>; 4] =
            std::array::from_fn(|_| broadcast::channel(cap).0);
        Bus {
            lanes: Arc::new(lanes),
            emitted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Fire a spike onto its lane. Returns the spike id. Delivery to zero listeners
    /// is not an error - a neuron that no one observes still fired.
    pub fn emit(&self, spike: Spike) -> u64 {
        let id = spike.id;
        let _ = self.lanes[spike.priority.index()].send(Arc::new(spike));
        self.emitted.fetch_add(1, Ordering::Relaxed);
        id
    }

    /// Total spikes emitted since boot - drives the "neurons firing" view.
    pub fn emitted(&self) -> u64 {
        self.emitted.load(Ordering::Relaxed)
    }

    /// Subscribe a new [`Synapse`]. Only spikes emitted *after* this call are seen.
    pub fn synapse(&self) -> Synapse {
        Synapse {
            rx: std::array::from_fn(|i| self.lanes[i].subscribe()),
        }
    }
}

/// A subscriber's connection to the bus. Yields spikes highest-priority-lane first.
pub struct Synapse {
    rx: [broadcast::Receiver<Arc<Spike>>; 4],
}

impl Synapse {
    /// Receive the next spike, draining higher-priority lanes before lower ones.
    ///
    /// Returns `None` once the bus is gone (all senders dropped). Spikes missed
    /// because this subscriber fell behind (lane overflow) are skipped, not errored.
    pub async fn recv(&mut self) -> Option<Arc<Spike>> {
        use broadcast::error::RecvError;
        loop {
            let [r0, r1, r2, r3] = &mut self.rx;
            let res = tokio::select! {
                biased;
                m = r0.recv() => m,
                m = r1.recv() => m,
                m = r2.recv() => m,
                m = r3.recv() => m,
            };
            match res {
                Ok(spike) => return Some(spike),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn delivers_emitted_spike() {
        let bus = Bus::new(16);
        let mut syn = bus.synapse();
        bus.emit(Spike::new("t.hello", Priority::Normal, json!({ "x": 1 })));
        let s = syn.recv().await.unwrap();
        assert_eq!(s.topic, "t.hello");
        assert_eq!(s.payload["x"], 1);
        assert_eq!(bus.emitted(), 1);
    }

    #[tokio::test]
    async fn reflex_lane_preempts_low() {
        let bus = Bus::new(16);
        let mut syn = bus.synapse();
        // Emit low first; a later reflex must still be delivered first.
        bus.emit(Spike::new("slow", Priority::Low, json!(null)));
        bus.emit(Spike::new("stop", Priority::Reflex, json!(null)));
        assert_eq!(syn.recv().await.unwrap().priority, Priority::Reflex);
        assert_eq!(syn.recv().await.unwrap().priority, Priority::Low);
    }

    #[tokio::test]
    async fn none_after_bus_dropped() {
        let bus = Bus::new(4);
        let mut syn = bus.synapse();
        drop(bus);
        assert!(syn.recv().await.is_none());
    }

    #[test]
    fn taint_is_monotonic() {
        assert_eq!(Taint::Trusted.join(Taint::Trusted), Taint::Trusted);
        assert!(Taint::Trusted.join(Taint::Untrusted).is_untrusted());
        assert!(Taint::Untrusted.join(Taint::Trusted).is_untrusted());
        assert!(Spike::new("x", Priority::Normal, json!(null))
            .tainted(Taint::Untrusted)
            .taint
            .is_untrusted());
    }

    #[test]
    fn priority_orders_reflex_first() {
        assert!(Priority::Reflex < Priority::Low);
        assert_eq!(Priority::LANES[0], Priority::Reflex);
    }
}
