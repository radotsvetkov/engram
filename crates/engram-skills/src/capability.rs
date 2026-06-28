//! Capabilities - what a skill is allowed to touch.
//!
//! The sandbox is **deny-by-default**: a skill receives a host function only if its
//! signed manifest requests the matching capability. Importing anything ungranted
//! fails at instantiation, so an over-reaching skill never runs at all.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read from the memory broker (`recall`).
    MemoryRead,
    /// Write to the memory broker (`remember`). Writes inherit the run's taint.
    MemoryWrite,
    /// Call a language model through the gateway.
    Llm,
    /// Reach the network directly.
    Net,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::MemoryRead => "memory_read",
            Capability::MemoryWrite => "memory_write",
            Capability::Llm => "llm",
            Capability::Net => "net",
        }
    }

    /// Capabilities that can carry data *out* of the process. These are revoked for a
    /// run that has read untrusted input - the no-egress half of the taint rule.
    pub fn is_egress(self) -> bool {
        matches!(self, Capability::Llm | Capability::Net)
    }

    /// Capabilities a skill may use only on a *trusted* run. A skill that reaches the network or a
    /// model can be steered into exfiltration or SSRF if the run has read untrusted content, so such
    /// a skill is refused on a tainted run at the skill-tool boundary (the central dispatch gate only
    /// covers egress *tools*, not a code-executing skill, so this is the explicit complement).
    pub fn requires_trust(self) -> bool {
        self.is_egress()
    }
}
