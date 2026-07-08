//! The deck: a single SQLite file holding the 考研 dictionary + FSRS card state.
//! Built once from ECDICT (`tuna build-deck`), then read/updated at study time.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rs_fsrs::{Card, Rating, ReviewLog};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::scheduler::state_from_i64;
use crate::llm::enrich::Enrichment;

/// One scoped dictionary row — the shippable form of a 考研 word. This is what we
/// bake into the committed/embedded `deck.jsonl` so other devices need no ECDICT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictRow {
    pub word: String,
    #[serde(default)]
    pub sw: String,
    #[serde(default)]
    pub phonetic: String,
    #[serde(default)]
    pub translation: String,
    #[serde(default)]
    pub definition: String,
    #[serde(default)]
    pub pos: String,
    #[serde(default)]
    pub collins: i64,
    #[serde(default)]
    pub oxford: i64,
    pub bnc: Option<i64>,
    pub frq: Option<i64>,
    #[serde(default)]
    pub exchange: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub priority: i64,
}

/// The 11 card columns, in the order `row_to_deckcard` expects.
const CARD_COLS: &str = "c.word, c.due, c.stability, c.difficulty, c.elapsed_days, \
     c.scheduled_days, c.reps, c.lapses, c.state, c.last_review, c.introduced";

/// Parse ECDICT's `exchange` field (e.g. "p:ran/d:run/i:running/3:runs/0:run")
/// into labelled word-family forms — real graph data we have before any LLM.
pub fn parse_exchange(exchange: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for part in exchange.split('/') {
        let Some((code, form)) = part.split_once(':') else {
            continue;
        };
        let label = match code {
            "p" => "过去式",
            "d" => "过去分词",
            "i" => "现在分词",
            "3" => "三单",
            "r" => "比较级",
            "t" => "最高级",
            "s" => "复数",
            "0" => "原形",
            "1" => "词根", // lemma transformation flags; shown as a link cue
            _ => continue,
        };
        if !form.trim().is_empty() {
            out.push((label, form.trim().to_string()));
        }
    }
    out
}

/// Canonical morpheme id from a surface: lowercase + trim, KEEPING the hyphen so a
/// suffix `-al` never merges with a prefix `al-` (must match scripts/narrate.py norm()).
fn normalize_morpheme(unit: &str) -> String {
    unit.trim().to_lowercase()
}

/// Dictionary facts surfaced in the UI.
#[derive(Debug, Clone)]
pub struct DictEntry {
    pub word: String,
    pub phonetic: String,
    pub translation: String,
    pub definition: String,
    pub pos: String,
    pub collins: i64,
    pub exchange: String,
    pub tag: String,
}

/// A card joined with just enough for scheduling + phase logic.
#[derive(Debug, Clone)]
pub struct DeckCard {
    pub word: String,
    pub introduced: bool,
    pub card: Card,
}

/// A word hanging off a morpheme node, for the constellation view.
#[derive(Debug, Clone)]
pub struct GraphMember {
    pub word: String,
    pub introduced: bool,
    pub stability: f64,
}

/// A morpheme hub + the words orbiting it.
#[derive(Debug, Clone)]
pub struct MorphemeGroup {
    pub surface: String,
    pub gloss_zh: String,
    pub members: Vec<GraphMember>,
}

/// Closed set of English derivational/inflectional endings. A word sharing one of
/// these with another word is grammar, not a derivation bond — kept out of the
/// constellation. Matched against the de-hyphenated surface so a bare `ion` (from an
/// inconsistently hyphenated bake) is caught the same as `-ion`.
fn is_grammatical_suffix(core: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        "ion", "tion", "sion", "ation", "ition", "ive", "ative", "itive", "ate", "ous",
        "ious", "eous", "ful", "less", "ness", "ment", "ity", "ety", "cy", "ance",
        "ence", "ancy", "ency", "able", "ible", "ial", "ical", "ically", "ism", "ist",
        "ize", "ise", "ify", "ing", "ish", "like", "ward", "wards", "wise", "hood",
        "ship", "dom", "age", "ery", "ary", "ory", "ling", "some", "teen",
    ];
    SUFFIXES.contains(&core)
}

/// A learned sibling that could anchor a new word — carried with its FSRS card so
/// the earned-edge engine can score it by retrievability and refresh it on recall.
#[derive(Debug, Clone)]
pub struct AnchorCand {
    pub word: String,
    pub morpheme_id: String,
    pub surface: String,
    pub gloss_zh: String,
    pub members: i64,
    pub card: Card,
}

#[derive(Debug, Default, Clone)]
pub struct DeckStats {
    pub words: i64,
    pub cards: i64,
    pub new: i64,
    pub introduced: i64,
    pub due_now: i64,
}

pub struct Deck {
    conn: Connection,
}

impl Deck {
    /// Open (creating + migrating) the deck at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening deck at {}", path.display()))?;
        conn.execute_batch(super::schema::SCHEMA)
            .context("applying schema")?;
        Ok(Self { conn })
    }

    /// (Re)build the deck from an ECDICT SQLite file: select every 考研-tagged word,
    /// copy its facts, and seed a fresh FSRS "New" card for each. Returns the count.
    pub fn build_from_ecdict(&mut self, ecdict_path: &Path) -> Result<usize> {
        anyhow::ensure!(
            ecdict_path.exists(),
            "ECDICT database not found at {} — download it first (see README).",
            ecdict_path.display()
        );
        let now = Utc::now();

        // The graph schema evolved (edge.via → why_zh; morpheme spine added).
        // Regenerate, don't migrate — the old free-text `via` is too dirty to salvage.
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS edge;
             DROP TABLE IF EXISTS morpheme;
             DROP TABLE IF EXISTS word_morpheme;",
        )?;
        self.conn.execute_batch(super::schema::SCHEMA)?;

        // ATTACH must run outside a transaction.
        self.conn
            .execute(
                "ATTACH DATABASE ?1 AS ecdict",
                params![ecdict_path.to_string_lossy()],
            )
            .context("attaching ECDICT")?;

        let result = (|| -> Result<usize> {
            let tx = self.conn.transaction()?;
            tx.execute("DELETE FROM card", [])?;
            tx.execute("DELETE FROM dict", [])?;

            // ky == 考研. priority = frequency rank (COCA, else BNC), unranked last.
            let n = tx.execute(
                r#"INSERT OR REPLACE INTO dict
                     (word, sw, phonetic, translation, definition, pos, collins, oxford, bnc, frq, exchange, tag, priority)
                   SELECT word, sw, phonetic, translation, definition, pos,
                          COALESCE(collins, 0), COALESCE(oxford, 0), bnc, frq, exchange, tag,
                          CASE WHEN COALESCE(frq,0) > 0 THEN frq
                               WHEN COALESCE(bnc,0) > 0 THEN bnc
                               ELSE 999999 END
                   FROM ecdict.stardict
                   WHERE tag LIKE '%ky%' AND word IS NOT NULL AND word != ''"#,
                [],
            )?;

            // One fresh New card per word (FSRS defaults: all zeros, due now).
            tx.execute(
                r#"INSERT OR REPLACE INTO card
                     (word, due, stability, difficulty, elapsed_days, scheduled_days, reps, lapses, state, last_review, introduced)
                   SELECT word, ?1, 0, 0, 0, 0, 0, 0, 0, ?1, 0 FROM dict"#,
                params![now],
            )?;

            tx.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('built_at', ?1), ('source', ?2)",
                params![now.to_rfc3339(), ecdict_path.to_string_lossy()],
            )?;
            tx.commit()?;
            Ok(n)
        })();

        // Always detach, even on error.
        let _ = self.conn.execute("DETACH DATABASE ecdict", []);
        result
    }

    /// Export the scoped dictionary to jsonl (maintainer step → committed asset).
    pub fn export_deck_jsonl(&self, out: &Path) -> Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT word, sw, phonetic, translation, definition, pos, collins, oxford,
                    bnc, frq, exchange, tag, priority
             FROM dict ORDER BY priority ASC, word ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(DictRow {
                word: r.get(0)?,
                sw: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                phonetic: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                translation: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                definition: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                pos: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                collins: r.get::<_, Option<i64>>(6)?.unwrap_or(0),
                oxford: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                bnc: r.get(8)?,
                frq: r.get(9)?,
                exchange: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                tag: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
                priority: r.get::<_, Option<i64>>(12)?.unwrap_or(999999),
            })
        })?;
        let mut buf = String::new();
        let mut n = 0;
        for row in rows {
            buf.push_str(&serde_json::to_string(&row?)?);
            buf.push('\n');
            n += 1;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, buf)?;
        Ok(n)
    }

    /// Build the deck from the embedded/committed `deck.jsonl` — the portable path,
    /// no ECDICT required. Regenerates the graph tables (schema-evolution safe).
    pub fn build_from_asset(&mut self, deck_jsonl: &str) -> Result<usize> {
        let now = Utc::now();
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS edge;
             DROP TABLE IF EXISTS morpheme;
             DROP TABLE IF EXISTS word_morpheme;",
        )?;
        self.conn.execute_batch(super::schema::SCHEMA)?;

        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM card", [])?;
        tx.execute("DELETE FROM dict", [])?;
        let mut n = 0;
        {
            let mut ins_dict = tx.prepare(
                "INSERT OR REPLACE INTO dict
                   (word, sw, phonetic, translation, definition, pos, collins, oxford, bnc, frq, exchange, tag, priority)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            let mut ins_card = tx.prepare(
                "INSERT OR REPLACE INTO card
                   (word, due, stability, difficulty, elapsed_days, scheduled_days, reps, lapses, state, last_review, introduced)
                 VALUES (?1, ?2, 0,0,0,0,0,0,0, ?2, 0)",
            )?;
            for line in deck_jsonl.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let d: DictRow = serde_json::from_str(line)
                    .with_context(|| format!("parsing deck asset line: {line}"))?;
                ins_dict.execute(params![
                    d.word, d.sw, d.phonetic, d.translation, d.definition, d.pos,
                    d.collins, d.oxford, d.bnc, d.frq, d.exchange, d.tag, d.priority,
                ])?;
                ins_card.execute(params![d.word, now])?;
                n += 1;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('built_at', ?1), ('source', 'embedded-asset')",
            params![now.to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// Load enrichment from an in-memory jsonl string (embedded asset).
    pub fn load_enrichment_str(&self, jsonl: &str) -> Result<usize> {
        let mut n = 0;
        for line in jsonl.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(e) = serde_json::from_str::<Enrichment>(line) else {
                continue;
            };
            if self.has_word(&e.word)? && self.save_enrichment(&e, line).is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn stats(&self) -> Result<DeckStats> {
        let now = Utc::now();
        let mut s = DeckStats::default();
        s.words = self.conn.query_row("SELECT COUNT(*) FROM dict", [], |r| r.get(0))?;
        s.cards = self.conn.query_row("SELECT COUNT(*) FROM card", [], |r| r.get(0))?;
        s.new = self
            .conn
            .query_row("SELECT COUNT(*) FROM card WHERE state = 0", [], |r| r.get(0))?;
        s.introduced = self.conn.query_row(
            "SELECT COUNT(*) FROM card WHERE introduced = 1",
            [],
            |r| r.get(0),
        )?;
        s.due_now = self.conn.query_row(
            "SELECT COUNT(*) FROM card WHERE due <= ?1",
            params![now],
            |r| r.get(0),
        )?;
        Ok(s)
    }

    /// Look up the dictionary facts for a word.
    pub fn entry(&self, word: &str) -> Result<Option<DictEntry>> {
        let e = self
            .conn
            .query_row(
                "SELECT word, phonetic, translation, definition, pos, collins, exchange, tag
                 FROM dict WHERE word = ?1",
                params![word],
                |r| {
                    Ok(DictEntry {
                        word: r.get(0)?,
                        phonetic: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        translation: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        definition: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                        pos: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                        collins: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                        exchange: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                        tag: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    })
                },
            )
            .optional()?;
        Ok(e)
    }

    /// The next batch to study: due cards first (by due time), then not-yet-introduced
    /// New cards in priority (frequency) order.
    pub fn next_queue(&self, now: DateTime<Utc>, limit: usize) -> Result<Vec<DeckCard>> {
        let sql = format!(
            "SELECT {CARD_COLS}
             FROM card c JOIN dict d ON d.word = c.word
             WHERE c.due <= ?1
             ORDER BY c.introduced ASC, d.priority ASC, c.due ASC
             LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![now, limit as i64], row_to_deckcard)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A study session: first the due *reviews* (time-sensitive), then up to
    /// `new_limit` fresh words in frequency order. On day one this is just the
    /// new intake; later it clears the review backlog before introducing more.
    pub fn session_queue(
        &self,
        now: DateTime<Utc>,
        new_limit: usize,
        review_limit: usize,
    ) -> Result<Vec<DeckCard>> {
        let review_sql = format!(
            "SELECT {CARD_COLS}
             FROM card c JOIN dict d ON d.word = c.word
             WHERE c.introduced = 1 AND c.due <= ?1
             ORDER BY c.due ASC LIMIT ?2"
        );
        let new_sql = format!(
            "SELECT {CARD_COLS}
             FROM card c JOIN dict d ON d.word = c.word
             WHERE c.introduced = 0
             ORDER BY d.priority ASC LIMIT ?1"
        );
        let mut out = Vec::new();
        {
            let mut stmt = self.conn.prepare(&review_sql)?;
            let rows = stmt.query_map(params![now, review_limit as i64], row_to_deckcard)?;
            for r in rows {
                out.push(r?);
            }
        }
        {
            let mut stmt = self.conn.prepare(&new_sql)?;
            let rows = stmt.query_map(params![new_limit as i64], row_to_deckcard)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    /// Persist a card's new FSRS state (and whether it has been introduced).
    pub fn save_card(&self, word: &str, card: &Card, introduced: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE card SET due=?2, stability=?3, difficulty=?4, elapsed_days=?5,
                 scheduled_days=?6, reps=?7, lapses=?8, state=?9, last_review=?10, introduced=?11
             WHERE word=?1",
            params![
                word,
                card.due,
                card.stability,
                card.difficulty,
                card.elapsed_days,
                card.scheduled_days,
                card.reps,
                card.lapses,
                card.state as i64,
                card.last_review,
                introduced as i64,
            ],
        )?;
        Ok(())
    }

    /// The top `limit` words by frequency (for pre-synthesizing audio, etc.).
    pub fn top_words(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT word FROM dict ORDER BY priority ASC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Words not yet enriched, in frequency order (so we spend LLM budget on the
    /// words you'll meet soonest).
    pub fn words_to_enrich(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.word FROM dict d LEFT JOIN enrichment e ON e.word = d.word
             WHERE e.word IS NULL ORDER BY d.priority ASC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Store an enrichment verbatim, seed the morpheme spine (word_morpheme links +
    /// morpheme nodes), and record ONLY pairwise semantic edges. cognate_root is
    /// never stored — it is derived by JOIN in `learned_siblings`.
    pub fn save_enrichment(&self, e: &Enrichment, raw_json: &str) -> Result<()> {
        let now = Utc::now();
        self.conn.execute(
            "INSERT OR REPLACE INTO enrichment (word, json, decomposable, etymology_confidence, enriched_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![e.word, raw_json, e.decomposable as i64, e.etymology_confidence, now],
        )?;
        // Pairwise word↔word relations only (cognate_root is derived, never stored).
        for ge in &e.graph_edges {
            if ge.target.is_empty() || !matches!(ge.relation.as_str(), "synonym" | "antonym" | "confusable") {
                continue;
            }
            self.conn.execute(
                "INSERT OR IGNORE INTO edge (src, dst, relation, why_zh) VALUES (?1, ?2, ?3, ?4)",
                params![e.word, ge.target, ge.relation, ge.why_zh],
            )?;
        }
        // Seed the spine: each morpheme becomes a node, this word links to it.
        // P0 uses a unit-normalized id; P1 replaces these with Wiktionary-grounded
        // canonical nodes (spec/spect/spic → one id).
        for (i, m) in e.morphemes.iter().enumerate() {
            let id = normalize_morpheme(&m.unit);
            if id.is_empty() {
                continue;
            }
            self.conn.execute(
                "INSERT OR IGNORE INTO morpheme (id, surface, kind, gloss_zh, confidence)
                 VALUES (?1, ?2, ?3, ?4, 'seed')",
                params![id, m.unit, m.kind, m.meaning_zh],
            )?;
            self.conn.execute(
                "INSERT OR IGNORE INTO word_morpheme (word, morpheme_id, position, surface)
                 VALUES (?1, ?2, ?3, ?4)",
                params![e.word, id, i as i64, m.unit],
            )?;
        }
        Ok(())
    }

    /// Load a committed enrichment asset (jsonl, one Enrichment per line) into the deck.
    /// This is how baked content ships — build-deck calls it so the graph is populated
    /// with zero runtime LLM.
    pub fn load_enrichment_asset(&self, path: &Path) -> Result<usize> {
        if !path.exists() {
            return Ok(0);
        }
        let text = std::fs::read_to_string(path)?;
        let mut n = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let e: Enrichment = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if self.has_word(&e.word)? && self.save_enrichment(&e, line).is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// The local constellation: for each of `word`'s morphemes, the deck words that
    /// hang off that root — with their learned status + FSRS stability (for glow).
    /// Only shows real, cited shared-morpheme edges; nothing is inferred.
    pub fn constellation(&self, word: &str) -> Result<Vec<MorphemeGroup>> {
        let mut ids = self.conn.prepare(
            "SELECT wm.morpheme_id, COALESCE(m.surface, wm.surface), COALESCE(m.gloss_zh, '')
             FROM word_morpheme wm LEFT JOIN morpheme m ON m.id = wm.morpheme_id
             WHERE wm.word = ?1",
        )?;
        let morphs: Vec<(String, String, String)> = ids
            .query_map(params![word], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut out = Vec::new();
        for (id, surface, gloss_zh) in morphs {
            // A derivation bond is a shared ROOT (no hyphen) or a meaningful PREFIX
            // (trailing hyphen). A shared grammatical suffix is noise you can't build on.
            // Leading-hyphen catches most, but the bake hyphenated inconsistently (some
            // words carry a bare `ion`/`ate`), so gate the de-hyphenated form against the
            // known closed set of English derivational/inflectional endings too.
            let core: String = surface.chars().filter(|c| *c != '-').collect();
            if surface.starts_with('-') || core.len() < 3 || is_grammatical_suffix(&core) {
                continue;
            }
            let mut ms = self.conn.prepare(
                "SELECT wm.word, c.introduced, c.stability
                 FROM word_morpheme wm JOIN card c ON c.word = wm.word
                 WHERE wm.morpheme_id = ?1
                 ORDER BY c.introduced DESC, c.stability DESC, wm.word",
            )?;
            let members: Vec<GraphMember> = ms
                .query_map(params![id], |r| {
                    Ok(GraphMember {
                        word: r.get(0)?,
                        introduced: r.get::<_, i64>(1)? != 0,
                        stability: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // ≥2 words to be a family; skip a hyper-generic connective that would be a
            // wall of unrelated words instead of a legible root neighbourhood.
            if members.len() > 1 && members.len() <= 60 {
                out.push(MorphemeGroup {
                    surface,
                    gloss_zh,
                    members,
                });
            }
        }
        // Rarest roots first — a shared spect is a tighter bond than a shared re-.
        out.sort_by_key(|g| g.members.len());
        Ok(out)
    }

    /// A candidate anchor: a learned deck word sharing a root with the new word,
    /// carried with its FSRS card so the caller can score by retrievability.
    pub fn anchor_candidates(&self, word: &str) -> Result<Vec<AnchorCand>> {
        let mut stmt = self.conn.prepare(
            "SELECT wm2.word, wm1.morpheme_id,
                    COALESCE(m.surface, wm1.surface), COALESCE(m.gloss_zh, ''),
                    (SELECT COUNT(*) FROM word_morpheme w3 WHERE w3.morpheme_id = wm1.morpheme_id) AS members,
                    c.due, c.stability, c.difficulty, c.elapsed_days, c.scheduled_days,
                    c.reps, c.lapses, c.state, c.last_review
             FROM word_morpheme wm1
             JOIN word_morpheme wm2
                  ON wm2.morpheme_id = wm1.morpheme_id AND wm2.word <> wm1.word
             LEFT JOIN morpheme m ON m.id = wm1.morpheme_id
             JOIN card c ON c.word = wm2.word AND c.introduced = 1
             WHERE wm1.word = ?1",
        )?;
        let rows = stmt.query_map(params![word], |r| {
            Ok(AnchorCand {
                word: r.get(0)?,
                morpheme_id: r.get(1)?,
                surface: r.get(2)?,
                gloss_zh: r.get(3)?,
                members: r.get(4)?,
                card: Card {
                    due: r.get(5)?,
                    stability: r.get(6)?,
                    difficulty: r.get(7)?,
                    elapsed_days: r.get(8)?,
                    scheduled_days: r.get(9)?,
                    reps: r.get(10)?,
                    lapses: r.get(11)?,
                    state: state_from_i64(r.get(12)?),
                    last_review: r.get(13)?,
                },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Deck words you've ALREADY introduced that share a canonical morpheme with `word`
    /// — the attach-to-the-known mechanic, derived live by JOIN (never a stored pair).
    /// Grows as you learn: introducing a word instantly makes it a candidate everywhere.
    pub fn learned_siblings(&self, word: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT wm2.word, COALESCE(m.surface, wm1.surface)
             FROM word_morpheme wm1
             JOIN word_morpheme wm2
                  ON wm2.morpheme_id = wm1.morpheme_id AND wm2.word <> wm1.word
             JOIN card c ON c.word = wm2.word AND c.introduced = 1
             LEFT JOIN morpheme m ON m.id = wm1.morpheme_id
             WHERE wm1.word = ?1
             LIMIT 6",
        )?;
        let rows = stmt.query_map(params![word], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Read a word's enrichment, if it has been enriched.
    pub fn enrichment(&self, word: &str) -> Result<Option<Enrichment>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM enrichment WHERE word = ?1",
                params![word],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    /// Is this word in the 考研 deck? (enrichment requires it — FK to dict.)
    pub fn has_word(&self, word: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM dict WHERE word = ?1",
            params![word],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn enriched_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM enrichment", [], |r| r.get(0))?)
    }

    /// Append a review-log entry.
    pub fn log_review(&self, word: &str, rating: Rating, log: &ReviewLog) -> Result<()> {
        self.conn.execute(
            "INSERT INTO review_log (word, rating, state, elapsed_days, scheduled_days, review_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                word,
                rating as i64,
                log.state as i64,
                log.elapsed_days,
                log.scheduled_days,
                log.reviewed_date,
            ],
        )?;
        Ok(())
    }
}

fn row_to_deckcard(r: &rusqlite::Row) -> rusqlite::Result<DeckCard> {
    Ok(DeckCard {
        word: r.get(0)?,
        card: Card {
            due: r.get(1)?,
            stability: r.get(2)?,
            difficulty: r.get(3)?,
            elapsed_days: r.get(4)?,
            scheduled_days: r.get(5)?,
            reps: r.get(6)?,
            lapses: r.get(7)?,
            state: state_from_i64(r.get(8)?),
            last_review: r.get(9)?,
        },
        introduced: r.get::<_, i64>(10)? != 0,
    })
}
