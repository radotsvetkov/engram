//! The shared anti-hallucination discipline `dissent.rs` pioneered: a model must cite evidence by
//! number from a list it was actually shown, and any citation that doesn't map to a real offered
//! item is dropped rather than trusted. Extracted so every "the model must ground this claim in
//! specific numbered evidence" feature (today: dissent, memory contradiction-detection) shares one
//! proven parser instead of each hand-rolling its own - the exact discipline the project's biggest
//! named risk (decorative, unverifiable "intelligence") requires, applied consistently.

use std::collections::HashSet;

/// Parse a `"<TAG> <comma-separated 1-based numbers> | <reason>"` reply (the reason clause is
/// optional), keeping only numbers that are valid 1-based indices into a candidate list of size
/// `n`. `tag` should include any trailing punctuation the prompt asks for (e.g. `"CONFLICT:"`).
///
/// Returns `None` when the tag isn't present on the first line, or when every cited number was out
/// of range (a model that claims a match but points at nothing real gets no match, not a guess).
/// Returns the deduplicated indices in citation order, plus the trimmed reason (empty string if
/// none was given).
pub fn parse_cited_claim(reply: &str, tag: &str, n: usize) -> Option<(Vec<usize>, String)> {
    let line = reply.trim().lines().next()?.trim();
    let rest = line.strip_prefix(tag)?;
    let (nums, why) = rest.split_once('|').unwrap_or((rest, ""));
    let mut idxs = Vec::new();
    let mut seen = HashSet::new();
    for tok in nums.split(',') {
        if let Ok(k) = tok.trim().parse::<usize>() {
            if k >= 1 && k <= n && seen.insert(k) {
                idxs.push(k);
            }
        }
    }
    if idxs.is_empty() {
        return None;
    }
    Some((idxs, why.trim().to_string()))
}

/// Render a 1-based numbered listing of candidate strings, the format every citing prompt in this
/// codebase shows the model (`"1. ...\n2. ...\n"`), so callers don't hand-roll the same loop.
pub fn number_candidates<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for (i, text) in items.into_iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tag_means_no_claim() {
        assert!(parse_cited_claim("NONE", "CONFLICT:", 3).is_none());
        assert!(parse_cited_claim("nothing to see here", "CONFLICT:", 3).is_none());
        assert!(parse_cited_claim("", "CONFLICT:", 3).is_none());
    }

    #[test]
    fn keeps_only_in_range_citations() {
        let (idxs, why) = parse_cited_claim("CONFLICT: 1, 2, 9 | because reasons", "CONFLICT:", 3)
            .expect("should parse");
        assert_eq!(
            idxs,
            vec![1, 2],
            "9 is out of range for n=3 and must be dropped"
        );
        assert_eq!(why, "because reasons");
    }

    #[test]
    fn all_hallucinated_citations_yields_none() {
        assert!(parse_cited_claim("CONFLICT: 7, 99 | trust me", "CONFLICT:", 3).is_none());
    }

    #[test]
    fn deduplicates_repeated_citations_in_order() {
        let (idxs, _) = parse_cited_claim("CONFLICT: 2,2,1,2", "CONFLICT:", 3).unwrap();
        assert_eq!(idxs, vec![2, 1]);
    }

    #[test]
    fn missing_reason_is_empty_not_a_parse_failure() {
        let (idxs, why) = parse_cited_claim("CONFLICT: 1", "CONFLICT:", 3).unwrap();
        assert_eq!(idxs, vec![1]);
        assert_eq!(why, "");
    }

    #[test]
    fn number_candidates_matches_the_one_based_listing_format() {
        let listing = number_candidates(["first", "second"]);
        assert_eq!(listing, "1. first\n2. second\n");
    }
}
