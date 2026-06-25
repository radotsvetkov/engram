//! # Engram skills — programs that learn from use
//!
//! A skill is not a prompt; it is a small WASM program that runs in a
//! capability-sandboxed, fuel-bounded host and can be measured, versioned, and
//! rewritten toward a metric. This crate provides the substrate:
//!
//! - [`capability::Capability`] — the deny-by-default permission set.
//! - [`manifest`] — signed manifests binding a skill's bytes, capabilities, and
//!   metric, so a self-modifying agent never runs unsigned or escalated code.
//! - [`host::SkillHost`] — the sandbox: writes input into guest memory, runs the
//!   skill with only its granted host functions, and returns the output plus the
//!   instrumentation (fuel, host calls, logs) the learning loop will optimise on.
//!
//! Egress capabilities are revoked automatically for a run that read untrusted
//! input — the no-egress half of the taint rule, enforced at the sandbox boundary.

pub mod capability;
pub mod host;
pub mod manifest;

pub use capability::Capability;
pub use host::{Outcome, RunCtx, SkillError, SkillHost};
pub use manifest::{module_hash, verify, Manifest, ManifestError, SignedSkill, SkillSigner};
