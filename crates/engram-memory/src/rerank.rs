//! Diversity reranking (Maximal Marginal Relevance).
//!
//! The semantic arm can return several near-duplicate passages - especially once documents are
//! ingested as overlapping chunks. Returning five paraphrases of the same fact wastes the recall
//! budget and crowds out other relevant memories. MMR greedily picks the next candidate that best
//! balances *relevance to the query* against *novelty vs what's already chosen*, so the result set
//! covers more ground.

use crate::embed::cosine;

/// Greedily select up to `k` candidate ids by Maximal Marginal Relevance. Each candidate is
/// `(id, relevance, embedding)`. `lambda` in [0,1] trades relevance (1.0 = pure relevance, no
/// diversity) against novelty. Candidates are assumed already sorted by descending relevance; the
/// first pick is always the most relevant.
pub fn mmr(candidates: &[(i64, f32, Vec<f32>)], lambda: f32, k: usize) -> Vec<i64> {
    if candidates.is_empty() || k == 0 {
        return Vec::new();
    }
    let lambda = lambda.clamp(0.0, 1.0);
    let mut selected: Vec<usize> = Vec::with_capacity(k.min(candidates.len()));
    let mut remaining: Vec<usize> = (0..candidates.len()).collect();

    // Seed with the most relevant candidate.
    selected.push(remaining.remove(0));

    while selected.len() < k && !remaining.is_empty() {
        let mut best_pos = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for (pos, &ci) in remaining.iter().enumerate() {
            let relevance = candidates[ci].1;
            // Redundancy = the highest similarity to anything already chosen.
            let max_sim = selected
                .iter()
                .map(|&si| cosine(&candidates[ci].2, &candidates[si].2))
                .fold(f32::NEG_INFINITY, f32::max);
            let score = lambda * relevance - (1.0 - lambda) * max_sim;
            if score > best_score {
                best_score = score;
                best_pos = pos;
            }
        }
        selected.push(remaining.remove(best_pos));
    }
    selected.into_iter().map(|i| candidates[i].0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmr_demotes_a_near_duplicate_in_favor_of_novelty() {
        // a and b are near-identical (high cosine); c is distinct but slightly less relevant.
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.99, 0.01, 0.0];
        let c = vec![0.0, 1.0, 0.0];
        let cands = vec![(1i64, 0.90f32, a), (2i64, 0.89f32, b), (3i64, 0.80f32, c)];
        // Pure relevance (lambda=1) would pick the two near-dups first.
        assert_eq!(mmr(&cands, 1.0, 2), vec![1, 2]);
        // With diversity, the distinct candidate is preferred over the near-duplicate.
        assert_eq!(
            mmr(&cands, 0.6, 2),
            vec![1, 3],
            "MMR should pick the novel candidate over the near-duplicate"
        );
    }

    #[test]
    fn mmr_handles_empty_and_small_inputs() {
        assert!(mmr(&[], 0.7, 5).is_empty());
        let one = vec![(7i64, 0.5f32, vec![1.0, 0.0])];
        assert_eq!(mmr(&one, 0.7, 5), vec![7]);
    }
}
