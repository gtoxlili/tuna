//! Content baked into the binary so a fresh device needs zero downloads (except the
//! optional Kokoro model). The maintainer regenerates these in-repo; users get them
//! for free — no 851MB ECDICT, no separate data files.

/// The 考研 dictionary, scoped to 4801 words with all fields (from ECDICT).
pub const DECK: &str = include_str!("../assets/deck.jsonl");

/// Baked per-word enrichment (morphemes, derivation, examples, edges).
pub const ENRICHMENT: &str = include_str!("../assets/enrichment.jsonl");
