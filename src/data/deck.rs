//! The deck: a single SQLite file holding the 考研 dictionary + FSRS card state.
//! Built once from ECDICT (`tuna build-deck`), then read/updated at study time.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rs_fsrs::{Card, Rating, ReviewLog};
use rusqlite::{params, Connection, OptionalExtension};

use super::scheduler::state_from_i64;

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
        let mut stmt = self.conn.prepare(
            "SELECT c.word, c.due, c.stability, c.difficulty, c.elapsed_days, c.scheduled_days,
                    c.reps, c.lapses, c.state, c.last_review, c.introduced
             FROM card c JOIN dict d ON d.word = c.word
             WHERE c.due <= ?1
             ORDER BY c.introduced ASC, d.priority ASC, c.due ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now, limit as i64], row_to_deckcard)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
