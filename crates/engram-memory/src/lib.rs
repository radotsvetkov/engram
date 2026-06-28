//! # Engram memory - the brain on disk
//!
//! Hybrid, region-partitioned, tiered memory in a single embedded SQLite file.
//!
//! - [`store::Memory`] is the broker: `remember` and `recall`, where recall fuses
//!   **keyword** (FTS5/BM25) and **semantic** (vector cosine) results so paraphrased
//!   queries still hit - the recall-quality win over a keyword-only agent.
//! - [`region::Region`] partitions memory like brain regions; recall consults the
//!   regions that match the task.
//! - [`embed::Embedder`] abstracts embeddings; the bundled [`embed::TrigramHashEmbedder`]
//!   keeps the pipeline testable and the binary tiny, with a real transformer model
//!   plugging into the same trait via the gateway.
//!
//! Every mutation is recorded in the core [`engram_core::Ledger`] before it lands.

pub mod embed;
pub mod region;
pub mod static_embed;
pub mod store;

pub use embed::{cosine, Embedder, TrigramHashEmbedder};
pub use engram_core::Taint;
pub use region::Region;
pub use static_embed::StaticEmbedder;
pub use store::{Hit, Memory, MemoryError, Record, Stats, WriteReq};
