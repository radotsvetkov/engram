//! # Engram core - the reactive kernel
//!
//! This crate is the brainstem. It owns the primitives everything else fires
//! through:
//!
//! - [`event`] - the **neural substrate**: spikes flow across priority lanes on an
//!   in-process bus that exists *only while the core is awake*. No resident daemon.
//! - [`lifecycle`] - **wake/sleep**: the core runs while there is activity and exits
//!   to zero RAM after an idle window. This is the basis of Engram's near-zero-idle
//!   cost, the headline advantage over an always-on Python/Node agent.
//! - [`ledger`] - the **audit ledger**: an append-only, hash-chained, Ed25519-signed
//!   record of every state change. Built before anything mutates state so the whole
//!   system is tamper-evident and reversible by construction.
//!
//! Everything here is deliberately small. Capability comes from architecture and the
//! right primitives, not from lines of code.

pub mod autonomy;
pub mod event;
pub mod ledger;
pub mod lifecycle;

pub use autonomy::{
    sign_policy, verify_policy, ActionClass, AutonomyPolicy, EgressBudget, EgressDecision, EgressRule,
    PolicyError, SignedAutonomyPolicy,
};
pub use event::{now_ms, Bus, Priority, Spike, Synapse, Taint};
pub use ledger::{
    entries_from_file, verify_file, verifying_key_from_hex, Entry, Ledger, LedgerError,
};
pub use lifecycle::{run_until_idle, Activity, WakeReason};

/// Semantic version of the kernel, taken from the crate manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
