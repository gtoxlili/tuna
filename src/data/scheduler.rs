//! FSRS scheduling wrapped thin. FSRS is a *mirror*: it models memory (difficulty,
//! stability, retrievability) and reports when a card is next due — it does not
//! decide how you study. We map a grade to a rating and let it schedule.
//!
//! Grade semantics for the 拆·联·验 method (reasoning quality, not raw recall):
//!   Again = blanked · Hard = needed the hint · Good = derived with effort · Easy = derived instantly.

use chrono::{DateTime, Utc};
use rs_fsrs::{Card, Rating, RecordLog, SchedulingInfo, FSRS};

pub struct Scheduler {
    fsrs: FSRS,
}

impl Default for Scheduler {
    fn default() -> Self {
        // Default FSRS-4.5 weights + 0.9 target retention. Personal weights get
        // fitted offline once enough review history accrues (M5).
        Self {
            fsrs: FSRS::default(),
        }
    }
}

impl Scheduler {
    /// Apply a rating at `now`, returning the next card state + the review log entry.
    pub fn grade(&self, card: Card, rating: Rating, now: DateTime<Utc>) -> SchedulingInfo {
        self.fsrs.next(card, now, rating)
    }

    /// Preview what each of the four ratings would schedule — used to show the
    /// learner "Again 10m · Hard 1d · Good 3d · Easy 6d" before they grade.
    pub fn preview(&self, card: Card, now: DateTime<Utc>) -> RecordLog {
        self.fsrs.repeat(card, now)
    }
}

/// Map the stored `state` integer back to an FSRS `State`.
pub fn state_from_i64(v: i64) -> rs_fsrs::State {
    match v {
        1 => rs_fsrs::State::Learning,
        2 => rs_fsrs::State::Review,
        3 => rs_fsrs::State::Relearning,
        _ => rs_fsrs::State::New,
    }
}

/// Map a grade key (1..4) to an FSRS `Rating`.
pub fn rating_from_u8(v: u8) -> Option<Rating> {
    match v {
        1 => Some(Rating::Again),
        2 => Some(Rating::Hard),
        3 => Some(Rating::Good),
        4 => Some(Rating::Easy),
        _ => None,
    }
}
