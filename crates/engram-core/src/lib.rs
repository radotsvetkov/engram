//! # Engram core — the reactive kernel
//!
//! This crate is the brainstem. It owns the two primitives everything else fires
//! through:
//!
//! - [`event`] — the **neural substrate**: spikes flow across priority lanes on an
//!   in-process bus that exists *only while the core is awake*. No resident daemon.
//! - [`lifecycle`] — **wake/sleep**: the core runs while there is activity and exits
//!   to zero RAM after an idle window. This is the basis of Engram's near-zero-idle
//!   cost, the headline advantage over an always-on Python/Node agent.
//!
//! Everything here is deliberately small. Capability comes from architecture and the
//! right primitives, not from lines of code.

pub mod event;
pub mod lifecycle;

pub use event::{now_ms, Bus, Priority, Spike, Synapse, Taint};
pub use lifecycle::{run_until_idle, Activity, WakeReason};

/// Semantic version of the kernel, taken from the crate manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
