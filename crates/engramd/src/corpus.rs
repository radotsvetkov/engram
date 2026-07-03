//! The document corpus: turning an uploaded file into scoped, retrievable memory.
//!
//! Rather than a parallel vector store, a document's text is **chunked and stored as scoped
//! memories** - so it inherits everything the brain already does: project isolation (a project's
//! documents never surface in another project), the binary-quantized two-stage recall, dedup (re-
//! uploading the same file is a no-op), and the signed ledger. Retrieval is then automatic: the
//! chunks are just facts the normal scoped recall returns.
//!
//! Provenance: a chunk is sourced `document:<name>#<i>` and stored TRUSTED - the user deliberately
//! brought this file into *their own* project, so it is usable reference material (unlike text the
//! agent scraped from an attacker-controlled page mid-run, which stays untrusted). This is the
//! single-user-local trade-off; a shared/multi-tenant deployment would default these untrusted and
//! gate them behind an explicit "trust" action.

use engram_core::Scope;
use engram_memory::{Memory, Region, Taint, WriteReq};

/// Target chunk size in characters (~300 tokens) and the overlap carried between hard-split pieces
/// of an oversized paragraph, so a fact spanning a split boundary is still wholly present in one
/// chunk.
const TARGET_CHARS: usize = 1200;
const OVERLAP_CHARS: usize = 150;

/// A source marker prefix identifying a memory as a document chunk.
pub const DOC_SOURCE_PREFIX: &str = "document:";

/// Split document text into retrieval-sized chunks on paragraph boundaries, packing paragraphs up
/// to `TARGET_CHARS` and hard-splitting any single oversized paragraph with a small overlap. Pure,
/// deterministic, offline - no model call.
pub fn chunk_text(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let paras: Vec<&str> = text
        .split("\n\n")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let paras = if paras.is_empty() { vec![text] } else { paras };

    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for p in paras {
        // Hard-split an oversized paragraph on char boundaries, with overlap.
        if p.chars().count() > TARGET_CHARS {
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            let chars: Vec<char> = p.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let end = (i + TARGET_CHARS).min(chars.len());
                chunks.push(chars[i..end].iter().collect());
                if end == chars.len() {
                    break;
                }
                i = end.saturating_sub(OVERLAP_CHARS);
            }
            continue;
        }
        // Flush the current chunk if adding this paragraph would overflow it.
        if !cur.is_empty() && cur.chars().count() + p.chars().count() + 2 > TARGET_CHARS {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(p);
    }
    if !cur.trim().is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// Ingest a document's extracted text into the corpus as scoped memory chunks, returning how many
/// chunks landed. Idempotent by construction: dedup-on-write collapses a re-uploaded identical file
/// (same chunk text in the same scope) back onto the existing rows instead of duplicating them.
pub fn ingest_document(memory: &Memory, name: &str, text: &str, scope: &Scope) -> usize {
    let chunks = chunk_text(text);
    let mut stored = 0usize;
    for (i, chunk) in chunks.into_iter().enumerate() {
        let req = WriteReq::new(Region::Semantic, chunk)
            .source(format!("{DOC_SOURCE_PREFIX}{name}#{i}"))
            .importance(0.6)
            .taint(Taint::Trusted)
            .actor("upload")
            .scope(scope.clone());
        if memory.remember(req).is_ok() {
            stored += 1;
        }
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::ScopeCtx;
    use engram_memory::TrigramHashEmbedder;
    use std::sync::Arc;

    fn mem() -> (Memory, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(engram_core::Ledger::open(dir.path()).unwrap());
        let m = Memory::open(
            dir.path().join("b.db"),
            Arc::new(TrigramHashEmbedder::default()),
            ledger,
        )
        .unwrap();
        (m, dir)
    }

    #[test]
    fn chunks_pack_paragraphs_and_split_giants() {
        let small = "Para one.\n\nPara two.\n\nPara three.";
        assert_eq!(
            chunk_text(small).len(),
            1,
            "small paras pack into one chunk"
        );
        let giant = "x".repeat(TARGET_CHARS * 3);
        let cs = chunk_text(&giant);
        assert!(
            cs.len() >= 3,
            "a giant paragraph is hard-split: {}",
            cs.len()
        );
        assert!(cs.iter().all(|c| c.chars().count() <= TARGET_CHARS));
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn ingested_document_is_recalled_in_its_project_only() {
        let (m, _d) = mem();
        let text = "The Q3 budget for project Apollo is 2 million euros.\n\n\
                    The Apollo launch window opens in October.";
        let n = ingest_document(&m, "apollo.txt", text, &Scope::project("APOLLO"));
        assert!(n >= 1);
        // Recalled inside its own project…
        let in_apollo = m
            .recall_trusted_scoped(
                "budget",
                &[Region::Semantic],
                5,
                &ScopeCtx::project("APOLLO"),
            )
            .unwrap();
        assert!(
            in_apollo
                .iter()
                .any(|h| h.record.text.contains("2 million")),
            "the document is retrievable in its project"
        );
        // …but never in a different project.
        let in_other = m
            .recall_trusted_scoped(
                "budget",
                &[Region::Semantic],
                5,
                &ScopeCtx::project("OTHER"),
            )
            .unwrap();
        assert!(
            in_other.is_empty()
                || in_other
                    .iter()
                    .all(|h| !h.record.text.contains("2 million")),
            "another project must not see this document"
        );
        // Re-ingesting the identical file does not duplicate rows (dedup-on-write).
        let n2 = ingest_document(&m, "apollo.txt", text, &Scope::project("APOLLO"));
        let all = m
            .recall("Apollo", &[Region::Semantic], 50)
            .unwrap()
            .into_iter()
            .filter(|h| {
                h.record
                    .source
                    .as_deref()
                    .map(|s| s.starts_with(DOC_SOURCE_PREFIX))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(n, n2, "re-ingest chunk count is stable");
        assert!(
            all <= n,
            "re-ingesting the same file did not duplicate chunks (got {all})"
        );
    }
}
