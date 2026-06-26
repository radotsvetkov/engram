//! # Engram scheduler - focused automation that wakes from nothing
//!
//! - [`recur`] turns natural language ("every weekday at 9am") into a deterministic
//!   [`recur::Recurrence`] and computes the next fire - no model call, fully testable.
//! - [`sched::Scheduler`] persists jobs, surfaces what is due, and reschedules forward
//!   across sleep without stampeding, recording each change in the audit ledger.
//! - [`systemd`] emits the socket-activation and timer units that make zero-idle and
//!   scheduled wake real on a $5 VPS.

pub mod recur;
pub mod sched;
pub mod systemd;

pub use recur::{parse, ParseError, Recurrence};
pub use sched::{Job, SchedError, Scheduler};
