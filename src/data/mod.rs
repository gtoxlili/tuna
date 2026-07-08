//! Data layer: the 考研-scoped dictionary + FSRS card state in one SQLite file.
//!
//! Some of this surface (the scheduler, `save_card`/`log_review`, the card/entry
//! fields) is exercised by the review loop in M2; allow it to sit unused until then.
#![allow(dead_code)]

pub mod deck;
pub mod scheduler;
pub mod schema;

pub use deck::Deck;
pub use scheduler::Scheduler;
