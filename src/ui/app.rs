//! Study session state and logic. A synchronous model: the render loop drives it,
//! keys mutate it, and the earphone gate is re-polled on a ~1s cadence. No async
//! runtime needed for the review loop — background work (LLM, audio) arrives later
//! over channels, keeping this core simple and robust for daily use.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use crate::audio::coreaudio;
use crate::audio::player::{self, RoutedPlayer};
use crate::audio::tts::Tts;
use crate::config::Config;
use crate::data::deck::{DeckCard, DictEntry};
use crate::data::scheduler::rating_from_u8;
use crate::data::{Deck, Scheduler};
use crate::llm::enrich::Enrichment;

/// Introductions per session — the comfortable 2028 pace (leaves room for reviews).
const NEW_PER_SESSION: usize = 15;
const REVIEW_CAP: usize = 300;
const GATE_POLL: Duration = Duration::from_millis(1000);

#[derive(PartialEq, Clone, Copy)]
pub enum Stage {
    /// Question shown, meaning hidden — the effortful-recall gate.
    Prompt,
    /// Meaning revealed, awaiting a grade.
    Revealed,
}

pub struct GateStatus {
    pub open: bool,
    pub device: Option<String>,
}

pub struct CardView {
    pub dc: DeckCard,
    pub entry: DictEntry,
    /// DeepSeek enrichment (morphemes/derivation/graph), if this word has it.
    pub enrichment: Option<Enrichment>,
    /// Phase A (拆·联, first meeting) vs Phase B (验, retrieval).
    pub is_new: bool,
    pub stage: Stage,
}

pub struct App {
    pub deck: Deck,
    pub scheduler: Scheduler,
    pub needle: String,
    pub tts: Tts,
    /// Playback stream, held open only while the earphone is present.
    player: Option<RoutedPlayer>,
    /// Transient one-line audio feedback (e.g. "尚未合成发音").
    pub audio_msg: Option<String>,
    pub queue: Vec<DeckCard>,
    pub pos: usize,
    pub current: Option<CardView>,
    pub gate: GateStatus,
    last_gate_poll: Instant,
    pub session_new: u32,
    pub session_reviews: u32,
    pub session_total: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(deck_path: &Path, cfg: &Config) -> Result<Self> {
        let deck = Deck::open(deck_path)?;
        let queue = deck.session_queue(Utc::now(), NEW_PER_SESSION, REVIEW_CAP)?;
        let mut app = Self {
            deck,
            scheduler: Scheduler::default(),
            needle: cfg.gate.needle.clone(),
            tts: cfg.tts_engine(),
            player: None,
            audio_msg: None,
            session_total: queue.len(),
            queue,
            pos: 0,
            current: None,
            gate: GateStatus {
                open: false,
                device: None,
            },
            last_gate_poll: Instant::now() - GATE_POLL,
            session_new: 0,
            session_reviews: 0,
            should_quit: false,
        };
        app.poll_gate();
        app.load_current()?;
        Ok(app)
    }

    /// Load the card at `pos` (or `None` when the session is finished).
    fn load_current(&mut self) -> Result<()> {
        self.current = None;
        while self.pos < self.queue.len() {
            let dc = self.queue[self.pos].clone();
            if let Some(entry) = self.deck.entry(&dc.word)? {
                let enrichment = self.deck.enrichment(&dc.word).unwrap_or(None);
                self.current = Some(CardView {
                    is_new: !dc.introduced,
                    entry,
                    enrichment,
                    dc,
                    stage: Stage::Prompt,
                });
                return Ok(());
            }
            // No dict entry (shouldn't happen) — skip it.
            self.pos += 1;
        }
        Ok(())
    }

    /// Load a specific word as the current Phase-A card (for previews/tests).
    pub fn force_card(&mut self, word: &str) -> Result<bool> {
        if let Some(entry) = self.deck.entry(word)? {
            let enrichment = self.deck.enrichment(word).unwrap_or(None);
            self.current = Some(CardView {
                dc: DeckCard {
                    word: word.to_string(),
                    introduced: false,
                    card: rs_fsrs::Card::new(),
                },
                entry,
                enrichment,
                is_new: true,
                stage: Stage::Prompt,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn done(&self) -> bool {
        self.current.is_none()
    }

    pub fn remaining(&self) -> usize {
        self.session_total.saturating_sub(self.pos)
    }

    pub fn on_key(&mut self, key: char) -> Result<()> {
        self.audio_msg = None;
        match key {
            'q' => self.should_quit = true,
            '\n' | '\r' => {
                if let Some(c) = &mut self.current {
                    if c.stage == Stage::Prompt {
                        c.stage = Stage::Revealed;
                    }
                }
            }
            ' ' => self.play_audio(),
            g @ '1'..='4' => {
                let is_revealed = matches!(
                    self.current.as_ref().map(|c| c.stage),
                    Some(Stage::Revealed)
                );
                if is_revealed {
                    self.grade(g as u8 - b'0')?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Play the current word's pronunciation — only through the bound earphone.
    fn play_audio(&mut self) {
        let Some(word) = self.current.as_ref().map(|c| c.entry.word.clone()) else {
            return;
        };
        if !self.gate.open {
            self.audio_msg = Some("耳机未连接 · 静默".to_string());
            return;
        }
        let path = self.tts.cache_path(&word);
        if !path.exists() {
            self.audio_msg = Some("尚未合成发音（tuna synth）".to_string());
            return;
        }
        if self.ensure_player() {
            if let Some(p) = &self.player {
                match p.play_file(&path) {
                    Ok(()) => self.audio_msg = Some(format!("♪ {word}")),
                    Err(_) => self.audio_msg = Some("播放失败".to_string()),
                }
            }
        } else {
            self.audio_msg = Some("无法打开耳机输出".to_string());
        }
    }

    /// Ensure a playback stream bound to the earphone exists. Returns whether one is ready.
    fn ensure_player(&mut self) -> bool {
        if self.player.is_some() {
            return true;
        }
        if let Some(device) = player::find_output_device(&self.needle) {
            if let Ok(p) = RoutedPlayer::open(device) {
                self.player = Some(p);
                return true;
            }
        }
        false
    }

    fn grade(&mut self, n: u8) -> Result<()> {
        let Some(rating) = rating_from_u8(n) else {
            return Ok(());
        };
        if let Some(c) = self.current.take() {
            let was_new = c.is_new;
            let info = self.scheduler.grade(c.dc.card.clone(), rating, Utc::now());
            self.deck.save_card(&c.dc.word, &info.card, true)?;
            self.deck.log_review(&c.dc.word, rating, &info.review_log)?;
            if was_new {
                self.session_new += 1;
            } else {
                self.session_reviews += 1;
            }
        }
        self.pos += 1;
        self.load_current()
    }

    /// For the revealed card, the interval each grade would schedule
    /// (Again / Hard / Good / Easy) — so the learner grades informed.
    pub fn interval_hints(&self) -> Option<[String; 4]> {
        let c = self.current.as_ref()?;
        let now = Utc::now();
        let log = self.scheduler.preview(c.dc.card.clone(), now);
        let ratings = [
            rs_fsrs::Rating::Again,
            rs_fsrs::Rating::Hard,
            rs_fsrs::Rating::Good,
            rs_fsrs::Rating::Easy,
        ];
        let mut out = [String::new(), String::new(), String::new(), String::new()];
        for (i, r) in ratings.iter().enumerate() {
            if let Some(info) = log.get(r) {
                out[i] = human_interval(info.card.due - now);
            }
        }
        Some(out)
    }

    /// Re-poll the earphone gate at most once per `GATE_POLL`.
    pub fn poll_gate(&mut self) {
        if self.last_gate_poll.elapsed() < GATE_POLL {
            return;
        }
        self.last_gate_poll = Instant::now();
        let open_device = match coreaudio::enumerate() {
            Ok(devices) => coreaudio::find_bound_output(&devices, &self.needle).map(|d| d.name.clone()),
            Err(_) => None,
        };
        self.gate = GateStatus {
            open: open_device.is_some(),
            device: open_device,
        };
        // Drop the playback stream the moment the earphone leaves — a mid-session
        // disconnect becomes instant silence, and we reopen on the next play.
        if !self.gate.open {
            self.player = None;
        }
    }
}

/// Render a `chrono::Duration` as a compact human interval (10m / 3h / 6d).
fn human_interval(d: chrono::Duration) -> String {
    let mins = d.num_minutes().max(0);
    if mins < 60 {
        format!("{}m", mins.max(1))
    } else if mins < 60 * 24 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / (60 * 24))
    }
}
