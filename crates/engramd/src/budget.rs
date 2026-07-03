//! The context-budget manager: fitting the assembled system context under a token ceiling.
//!
//! The run's standing context is built from several sources - the date, the agent's role, the
//! always-loaded consciousness, the active project's block, recalled memories, document chunks, the
//! skills catalogue, the persona. Left unbounded, a flood of recalled memories or a large ingested
//! document could crowd out (or blow past) the model's window. The packer keeps the highest-priority
//! parts whole and truncates or drops the lowest-priority ones, so the essentials always survive and
//! the budget is never exceeded.

/// A rough token estimate (~4 chars/token) - matches the gateway's `approx_tokens` heuristic and is
/// good enough for budgeting without a tokenizer dependency.
pub fn approx_tokens(s: &str) -> usize {
    s.chars().count() / 4 + 1
}

/// One piece of the system context, tagged with its priority tier (LOWER = more important). Tier 0
/// parts are essential and always kept whole; higher tiers are packed under the remaining budget.
pub struct Part {
    pub text: String,
    pub tier: u8,
}

impl Part {
    pub fn new(text: impl Into<String>, tier: u8) -> Self {
        Part {
            text: text.into(),
            tier,
        }
    }
}

/// Pack `parts` into a single system-context string under `max_tokens`. Tier-0 parts are always
/// included whole (they are small and essential - the date, role, and curated working memory). The
/// remaining parts are added in priority order until the budget is reached; the first part that
/// doesn't fit is truncated to what's left (with a marker) and packing stops, so lower-priority
/// context degrades gracefully instead of being silently over-stuffed.
pub fn pack(mut parts: Vec<Part>, max_tokens: usize) -> String {
    parts.sort_by_key(|p| p.tier); // stable: equal tiers keep insertion order
    let mut out: Vec<String> = Vec::new();
    let mut used = 0usize;
    for p in parts {
        if p.text.trim().is_empty() {
            continue;
        }
        let t = approx_tokens(&p.text);
        if p.tier == 0 {
            used += t; // essential: kept whole even if it alone is large
            out.push(p.text);
            continue;
        }
        if used + t <= max_tokens {
            used += t;
            out.push(p.text);
        } else {
            let remaining = max_tokens.saturating_sub(used);
            // Only bother truncating if a meaningful amount would survive.
            if remaining > 24 {
                let keep_chars = remaining * 4;
                let truncated: String = p.text.chars().take(keep_chars).collect();
                out.push(format!("{truncated}\n[…trimmed to fit the context budget]"));
            }
            break;
        }
    }
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn essentials_survive_a_flood_of_low_priority_context() {
        let essential = "SYSTEM: you are Engram.";
        let flood: String = "recalled noise. ".repeat(5000); // ~large
        let packed = pack(vec![Part::new(essential, 0), Part::new(flood, 2)], 500);
        assert!(
            packed.contains("you are Engram"),
            "the tier-0 essential must never be dropped"
        );
        assert!(
            approx_tokens(&packed) <= 500 + approx_tokens(essential) + 10,
            "the low-priority flood is trimmed to the budget"
        );
        assert!(
            packed.contains("trimmed to fit"),
            "overflow is marked, not silent"
        );
    }

    #[test]
    fn everything_fits_when_under_budget() {
        let packed = pack(
            vec![
                Part::new("alpha", 0),
                Part::new("beta", 1),
                Part::new("gamma", 2),
            ],
            10_000,
        );
        assert!(packed.contains("alpha") && packed.contains("beta") && packed.contains("gamma"));
    }

    #[test]
    fn higher_priority_wins_the_budget() {
        // A tier-1 part and a tier-2 part, only room for one: the tier-1 part wins.
        let packed = pack(
            vec![
                Part::new("PROJECT-CONTEXT", 1),
                Part::new("z".repeat(4000), 2),
            ],
            40,
        );
        assert!(packed.contains("PROJECT-CONTEXT"));
    }
}
