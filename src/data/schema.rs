//! The local SQLite schema. One file holds everything: the 考研-scoped dictionary
//! (read-mostly facts from ECDICT), each word's FSRS card state, the review log
//! (for your dashboard + later weight optimization), and a meta table.
//!
//! Morpheme/graph/enrichment tables arrive in M3; kept out here so the M1 data
//! layer stays a clean, verifiable core.

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

-- 考研 dictionary facts, sourced from ECDICT, scoped to the exam.
CREATE TABLE IF NOT EXISTS dict (
    word        TEXT PRIMARY KEY,      -- headword
    sw          TEXT,                  -- stripped search form
    phonetic    TEXT,                  -- IPA
    translation TEXT,                  -- ZH gloss
    definition  TEXT,                  -- EN definition
    pos         TEXT,                  -- part-of-speech ratios
    collins     INTEGER DEFAULT 0,     -- 0..5 commonness stars
    oxford      INTEGER DEFAULT 0,     -- 0/1 core-3000 flag
    bnc         INTEGER,               -- BNC frequency rank (lower = more frequent)
    frq         INTEGER,               -- COCA frequency rank
    exchange    TEXT,                  -- inflection/derivation codes (p:/d:/i:/3:/...)
    tag         TEXT,                  -- exam bands (…ky… = 考研)
    priority    INTEGER                -- computed introduction order (lower = earlier)
);

-- FSRS spaced-repetition state, one row per card.
CREATE TABLE IF NOT EXISTS card (
    word           TEXT PRIMARY KEY REFERENCES dict(word) ON DELETE CASCADE,
    due            TEXT NOT NULL,          -- RFC3339
    stability      REAL NOT NULL DEFAULT 0,
    difficulty     REAL NOT NULL DEFAULT 0,
    elapsed_days   INTEGER NOT NULL DEFAULT 0,
    scheduled_days INTEGER NOT NULL DEFAULT 0,
    reps           INTEGER NOT NULL DEFAULT 0,
    lapses         INTEGER NOT NULL DEFAULT 0,
    state          INTEGER NOT NULL DEFAULT 0,  -- 0 New / 1 Learning / 2 Review / 3 Relearning
    last_review    TEXT NOT NULL,          -- RFC3339
    introduced     INTEGER NOT NULL DEFAULT 0   -- has the word passed Phase A (拆·联)?
);
CREATE INDEX IF NOT EXISTS card_due ON card(due);
CREATE INDEX IF NOT EXISTS card_state ON card(state);

-- Review history: every grade, for the dashboard and offline weight fitting.
CREATE TABLE IF NOT EXISTS review_log (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    word           TEXT NOT NULL,
    rating         INTEGER NOT NULL,       -- 1 Again / 2 Hard / 3 Good / 4 Easy
    state          INTEGER NOT NULL,
    elapsed_days   INTEGER NOT NULL,
    scheduled_days INTEGER NOT NULL,
    review_ts      TEXT NOT NULL           -- RFC3339
);
CREATE INDEX IF NOT EXISTS rlog_word ON review_log(word);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- DeepSeek enrichment: the full JSON stored verbatim, plus a couple of columns
-- pulled out for filtering. (M3)
CREATE TABLE IF NOT EXISTS enrichment (
    word                 TEXT PRIMARY KEY REFERENCES dict(word) ON DELETE CASCADE,
    json                 TEXT NOT NULL,
    decomposable         INTEGER,
    etymology_confidence TEXT,      -- solid / folk / mnemonic
    enriched_at          TEXT
);

-- Knowledge-graph edges (from morpheme cognates + the LLM's graph_edges).
CREATE TABLE IF NOT EXISTS edge (
    src      TEXT NOT NULL,          -- the enriched word
    dst      TEXT NOT NULL,          -- the related word
    relation TEXT NOT NULL,          -- cognate_root / synonym / antonym / confusable
    via      TEXT,                   -- the shared morpheme, when relation = cognate_root
    PRIMARY KEY (src, dst, relation)
);
CREATE INDEX IF NOT EXISTS edge_src ON edge(src);
CREATE INDEX IF NOT EXISTS edge_dst ON edge(dst);
"#;
