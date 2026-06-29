//! Signed autonomy policy — the standing, pre-authorized grant that lets an agent run UNATTENDED for
//! days without a human approving each egress.
//!
//! The one-time human-approval flag ([`crate::Taint`]-gated egress, cleared by a UI "Approve once"
//! click) couples a *safety* decision to a *liveness* assumption: it only resolves if a human is
//! watching. For a days-long run nobody is — so the gate deadlocks. The fix is to move the human
//! moment OUT of the run: the human signs a policy ONCE, ahead of time, and the runtime evaluates it
//! deterministically thereafter.
//!
//! A policy is a default-DENY allowlist of egress destinations + permitted action classes + a
//! self-depleting [`EgressBudget`] (max actions, spend cap, expiry) + a hardline floor that no policy
//! can lift. It is Ed25519-signed with the same key family that signs skills and the ledger, so an
//! unsigned/forged policy fails closed (treated as empty), and the bypass cannot be flipped by
//! prompt-injected content at runtime. The egress gate calls [`AutonomyPolicy::resolve`] to get one
//! of three synchronously-computable outcomes — proceed, stage (park for async review), or refuse.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The class of an outbound action, matched against a policy's `allowed_actions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    /// Direct message / email to a recipient.
    Send,
    /// Public or semi-public post.
    Post,
    /// Anything that moves money.
    Pay,
    /// Any other egress (an opaque MCP tool, a generic webhook).
    Other,
}

/// One allowlist (or floor) entry, matched against an egress destination (a host or a recipient).
/// Glob forms mirror Hermes's host matching: `*` = any, `*.example.com` / `.example.com` = that
/// domain and its subdomains, otherwise an exact (case-insensitive) match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressRule {
    pub pattern: String,
}

impl EgressRule {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    pub fn matches(&self, dest: &str) -> bool {
        // Canonicalize away a trailing FQDN dot on BOTH sides: `paste.evil.com.` resolves to the same
        // host as `paste.evil.com`, so without this a dotted name slips past a suffix floor while
        // still matching a broad `*` allowlist — defeating the "floor governs all egress" invariant.
        let p = self.pattern.trim().trim_end_matches('.').to_ascii_lowercase();
        let d = dest.trim().trim_end_matches('.').to_ascii_lowercase();
        if p == "*" {
            return true;
        }
        // `*.suffix` / `.suffix` — the suffix host itself or any subdomain of it.
        let suffix = p.strip_prefix("*.").or_else(|| p.strip_prefix('.'));
        if let Some(suf) = suffix {
            return d == suf || d.ends_with(&format!(".{suf}"));
        }
        d == p
    }
}

/// A self-depleting authorization envelope. Never waits on a human: a counter is checked, money is
/// capped, and the grant expires — all evaluated against signed state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressBudget {
    /// Max number of egress actions this policy authorizes.
    pub max_actions: u32,
    /// RESERVED — a spend cap in cents. NOT yet enforced: `resolve()` does not read it and no
    /// per-action amount is threaded, so do not present it as a guarantee. Pay-class actions are
    /// governed today only by `allowed_actions` + the destination allowlist. (0 = unset.)
    #[serde(default)]
    pub max_spend_cents: u64,
    /// Unix-epoch ms after which the grant is dead (0 = no expiry).
    #[serde(default)]
    pub expires_at_ms: u64,
}

/// The synchronous decision the egress gate acts on. `Allow` means the action passed the floor,
/// allowlist, action-class and expiry checks — the caller must still atomically claim a budget slot
/// (so concurrent fan-out can't overspend). The `&'static str` on `Stage`/`Refuse` is a reason code
/// for the audit ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressDecision {
    Allow,
    Stage(&'static str),
    Refuse(&'static str),
}

/// A standing, signed authorization for an agent or scheduled task to act unattended.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyPolicy {
    /// What this policy authorizes (an agent id or task id) — for the audit trail.
    pub scope: String,
    /// Egress destinations the agent may reach unattended. Empty = nothing (default-deny).
    #[serde(default)]
    pub allowed_egress: Vec<EgressRule>,
    /// Action classes permitted. Empty = any class is allowed (so a destination allowlist alone
    /// suffices); non-empty restricts to the listed classes.
    #[serde(default)]
    pub allowed_actions: Vec<ActionClass>,
    pub budget: EgressBudget,
    /// Destinations that are NEVER allowed, no matter what the allowlist says (the floor).
    #[serde(default)]
    pub hardline_floor: Vec<EgressRule>,
}

impl AutonomyPolicy {
    pub fn max_actions(&self) -> u32 {
        self.budget.max_actions
    }

    /// The pure tier decision for one egress action. Does NOT mutate budget — on `Allow` the caller
    /// atomically claims a slot and re-checks the count, so the budget is correct under concurrency.
    ///
    /// Order matters: the floor is checked FIRST (it overrides everything), then expiry, then action
    /// class, then the allowlist. Anything not explicitly allowed is `Stage`d (parked for async
    /// review), never silently dropped — default-deny without becoming a hard wall.
    pub fn resolve(&self, dest: &str, class: ActionClass, now_ms: u64) -> EgressDecision {
        if self.hardline_floor.iter().any(|r| r.matches(dest)) {
            return EgressDecision::Refuse("egress_refused_floor");
        }
        if self.budget.expires_at_ms != 0 && now_ms >= self.budget.expires_at_ms {
            return EgressDecision::Stage("grant_expired");
        }
        if !self.allowed_actions.is_empty() && !self.allowed_actions.contains(&class) {
            return EgressDecision::Stage("action_not_allowed");
        }
        if self.allowed_egress.iter().any(|r| r.matches(dest)) {
            return EgressDecision::Allow;
        }
        EgressDecision::Stage("destination_not_allowlisted")
    }

    /// Canonical bytes for signing (serde_json sorts map keys deterministically).
    fn canonical(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// A policy plus its detached Ed25519 signature (hex). Persisted on the agent/job record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedAutonomyPolicy {
    pub policy: AutonomyPolicy,
    /// Hex Ed25519 signature over BLAKE3(canonical policy).
    pub sig: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("hex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("bad key material")]
    Key,
    #[error("signature invalid")]
    BadSignature,
}

/// Sign a policy, producing a record safe to persist. The human does this ONCE, while present.
pub fn sign_policy(policy: &AutonomyPolicy, signing: &SigningKey) -> SignedAutonomyPolicy {
    let digest = blake3::hash(&policy.canonical());
    let sig = signing.sign(digest.as_bytes());
    SignedAutonomyPolicy {
        policy: policy.clone(),
        sig: hex::encode(sig.to_bytes()),
    }
}

/// Verify a signed policy and return the policy it authorizes. An unsigned/forged/tampered policy is
/// an error — the caller treats that as "no policy" (default-deny), so the bypass fails closed.
pub fn verify_policy(
    signed: &SignedAutonomyPolicy,
    vk: &VerifyingKey,
) -> Result<AutonomyPolicy, PolicyError> {
    let digest = blake3::hash(&signed.policy.canonical());
    let sig_bytes: [u8; 64] = hex::decode(&signed.sig)?
        .try_into()
        .map_err(|_| PolicyError::Key)?;
    let sig = Signature::from_bytes(&sig_bytes);
    vk.verify(digest.as_bytes(), &sig)
        .map_err(|_| PolicyError::BadSignature)?;
    Ok(signed.policy.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> AutonomyPolicy {
        AutonomyPolicy {
            scope: "agent:assistant".into(),
            allowed_egress: vec![EgressRule::new("*.example.com"), EgressRule::new("boss@co.com")],
            allowed_actions: vec![ActionClass::Send],
            budget: EgressBudget {
                max_actions: 3,
                max_spend_cents: 0,
                expires_at_ms: 0,
            },
            hardline_floor: vec![EgressRule::new("*.evil.test")],
        }
    }

    #[test]
    fn rule_matching_globs_and_exact() {
        assert!(EgressRule::new("*").matches("anything.com"));
        assert!(EgressRule::new("*.example.com").matches("mail.example.com"));
        assert!(EgressRule::new("*.example.com").matches("example.com"));
        assert!(!EgressRule::new("*.example.com").matches("notexample.com"));
        assert!(EgressRule::new(".example.com").matches("a.b.example.com"));
        assert!(EgressRule::new("boss@co.com").matches("boss@co.com"));
        assert!(!EgressRule::new("boss@co.com").matches("evil@co.com"));
        assert!(EgressRule::new("Boss@CO.com").matches("boss@co.com")); // case-insensitive
    }

    #[test]
    fn resolve_tiers() {
        let p = policy();
        // Floor overrides everything.
        assert_eq!(p.resolve("x.evil.test", ActionClass::Send, 0), EgressDecision::Refuse("egress_refused_floor"));
        // Allowlisted host + allowed class -> Allow.
        assert_eq!(p.resolve("mail.example.com", ActionClass::Send, 0), EgressDecision::Allow);
        // Allowlisted recipient -> Allow.
        assert_eq!(p.resolve("boss@co.com", ActionClass::Send, 0), EgressDecision::Allow);
        // Not on the allowlist -> Stage (parked, not hard-refused).
        assert_eq!(p.resolve("random.org", ActionClass::Send, 0), EgressDecision::Stage("destination_not_allowlisted"));
        // Right destination, wrong action class -> Stage.
        assert_eq!(p.resolve("mail.example.com", ActionClass::Pay, 0), EgressDecision::Stage("action_not_allowed"));
    }

    #[test]
    fn trailing_dot_fqdn_cannot_evade_floor_or_suffix() {
        // A dotted FQDN resolves to the same host, so it must be caught by the floor and a suffix rule.
        assert!(EgressRule::new("paste.evil.com").matches("paste.evil.com."));
        assert!(EgressRule::new("*.evil.com").matches("paste.evil.com."));
        // The floor wins even under an allow-* policy (the bug was: floor missed it, `*` matched it).
        let p = AutonomyPolicy {
            scope: "agent:1".into(),
            allowed_egress: vec![EgressRule::new("*")],
            allowed_actions: vec![],
            budget: EgressBudget { max_actions: 10, max_spend_cents: 0, expires_at_ms: 0 },
            hardline_floor: vec![EgressRule::new("paste.evil.com")],
        };
        assert_eq!(
            p.resolve("paste.evil.com.", ActionClass::Send, 0),
            EgressDecision::Refuse("egress_refused_floor")
        );
        // Exact/email matching is unaffected (no trailing dot to strip).
        assert!(EgressRule::new("boss@co.com").matches("boss@co.com"));
    }

    #[test]
    fn resolve_honors_expiry() {
        let mut p = policy();
        p.budget.expires_at_ms = 1000;
        assert_eq!(p.resolve("mail.example.com", ActionClass::Send, 999), EgressDecision::Allow);
        assert_eq!(p.resolve("mail.example.com", ActionClass::Send, 1000), EgressDecision::Stage("grant_expired"));
    }

    #[test]
    fn empty_allowed_actions_permits_any_class() {
        let mut p = policy();
        p.allowed_actions.clear();
        assert_eq!(p.resolve("mail.example.com", ActionClass::Pay, 0), EgressDecision::Allow);
    }

    #[test]
    fn sign_and_verify_roundtrip_and_tamper_detection() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let vk = signing.verifying_key();
        let signed = sign_policy(&policy(), &signing);
        // Valid signature verifies and returns the policy.
        let back = verify_policy(&signed, &vk).unwrap();
        assert_eq!(back.scope, "agent:assistant");
        // Tampering with the policy after signing must fail (the floor can't be silently removed).
        let mut forged = signed.clone();
        forged.policy.hardline_floor.clear();
        forged.policy.allowed_egress.push(EgressRule::new("*"));
        assert!(matches!(verify_policy(&forged, &vk), Err(PolicyError::BadSignature)));
        // A different key cannot validate it.
        let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert!(verify_policy(&signed, &other).is_err());
    }
}
