//! Study session state and logic. A synchronous model: the render loop drives it,
//! keys mutate it, and the earphone gate is re-polled on a ~1s cadence. No async
//! runtime needed for the review loop — background work (LLM, audio) arrives later
//! over channels, keeping this core simple and robust for daily use.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use crate::audio::coreaudio;
use crate::audio::player::{self, RoutedPlayer};
use crate::audio::tts::{Tts, TtsServer};
use crate::config::Config;
use crate::data::deck::{DeckCard, DictEntry, MorphemeGroup};
use crate::data::scheduler::rating_from_u8;
use crate::data::{Deck, Scheduler};
use crate::llm::enrich::Enrichment;
use crate::llm::DeepSeek;

/// Introductions per session — the comfortable 2028 pace (leaves room for reviews).
const NEW_PER_SESSION: usize = 15;
const REVIEW_CAP: usize = 300;
const GATE_POLL: Duration = Duration::from_millis(1000);
const STRIKE_ANIM_MS: u128 = 900;

#[derive(PartialEq, Clone, Copy)]
pub enum Stage {
    /// Question shown, meaning hidden — the effortful-recall gate.
    Prompt,
    /// Meaning revealed, awaiting a grade.
    Revealed,
}

/// The earned-edge (星火接线) sub-interaction, only when a learned sibling exists.
#[derive(PartialEq, Clone, Copy)]
pub enum Strike {
    /// No anchor, or grading is unblocked.
    Idle,
    /// "Which learned word carries this root?" — anchor hidden, recall in your head.
    Prompt,
    /// Anchor revealed — did you remember it? (y / n)
    Flipped,
}

/// The chosen anchor: a learned sibling to recall, with its FSRS card to refresh.
#[derive(Clone)]
pub struct Anchor {
    pub word: String,
    pub surface: String,
    pub gloss_zh: String,
    pub card: rs_fsrs::Card,
}

pub struct GateStatus {
    pub open: bool,
    pub device: Option<String>,
}

/// State of the on-demand Socratic 辨析 popup.
pub enum Ask {
    Idle,
    Pending,
    Answer(String),
    Failed(String),
}

pub struct CardView {
    pub dc: DeckCard,
    pub entry: DictEntry,
    /// DeepSeek enrichment (morphemes/derivation/graph), if this word has it.
    pub enrichment: Option<Enrichment>,
    /// Deck words already learned that share a root — (word, via-morpheme).
    pub siblings: Vec<(String, String)>,
    /// The best learned sibling to attach this new word to (星火接线), if any.
    pub anchor: Option<Anchor>,
    pub strike: Strike,
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
    /// Warm Kokoro process, started lazily on first on-demand synth.
    tts_server: Arc<Mutex<Option<TtsServer>>>,
    tts_rx: Option<std::sync::mpsc::Receiver<std::result::Result<PathBuf, String>>>,
    /// The word currently being synthesized (drives the spinner).
    pub tts_pending: Option<String>,
    /// Transient one-line audio feedback.
    pub audio_msg: Option<String>,
    /// The learner's typed guess in the derive game (Phase A).
    pub input: String,
    /// Animation clock (spinners advance off this).
    pub anim: Instant,
    /// Set when the 星火接线 arc fires (successful recall) — drives a brief animation.
    pub strike_anim: Option<Instant>,
    pub queue: Vec<DeckCard>,
    pub pos: usize,
    pub current: Option<CardView>,
    pub gate: GateStatus,
    last_gate_poll: Instant,
    pub session_new: u32,
    pub session_reviews: u32,
    pub session_total: usize,
    pub should_quit: bool,
    /// Whether the constellation (root-family map) overlay is open.
    pub show_graph: bool,
    /// The current word's root-family, computed when the overlay opens.
    pub graph: Vec<MorphemeGroup>,
    // Socratic 辨析 (live DeepSeek on a worker thread)
    pub ask: Ask,
    ask_rx: Option<std::sync::mpsc::Receiver<std::result::Result<String, String>>>,
    ds_base: String,
    ds_key: String,
    ds_chat_model: String,
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
            tts_server: Arc::new(Mutex::new(None)),
            tts_rx: None,
            tts_pending: None,
            audio_msg: None,
            input: String::new(),
            anim: Instant::now(),
            strike_anim: None,
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
            show_graph: false,
            graph: Vec::new(),
            ask: Ask::Idle,
            ask_rx: None,
            ds_base: cfg.deepseek.base_url.clone(),
            ds_key: cfg.deepseek.api_key.clone(),
            ds_chat_model: cfg.deepseek.chat_model.clone(),
        };
        app.poll_gate();
        app.load_current()?;
        Ok(app)
    }

    /// Load the card at `pos` (or `None` when the session is finished).
    fn load_current(&mut self) -> Result<()> {
        self.current = None;
        self.input.clear();
        while self.pos < self.queue.len() {
            let dc = self.queue[self.pos].clone();
            if let Some(entry) = self.deck.entry(&dc.word)? {
                let enrichment = self.deck.enrichment(&dc.word).unwrap_or(None);
                let siblings = self.deck.learned_siblings(&dc.word).unwrap_or_default();
                let anchor = if dc.introduced {
                    None
                } else {
                    self.best_anchor(&dc.word)
                };
                self.current = Some(CardView {
                    is_new: !dc.introduced,
                    entry,
                    enrichment,
                    siblings,
                    anchor,
                    strike: Strike::Idle,
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
            let siblings = self.deck.learned_siblings(word).unwrap_or_default();
            let anchor = self.best_anchor(word);
            self.current = Some(CardView {
                dc: DeckCard {
                    word: word.to_string(),
                    introduced: false,
                    card: rs_fsrs::Card::new(),
                },
                entry,
                enrichment,
                siblings,
                anchor,
                strike: Strike::Idle,
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
        // The Socratic popup swallows input; 'a' or Esc closes it.
        if !matches!(self.ask, Ask::Idle) {
            if key == 'a' || key == '\x1b' {
                self.ask = Ask::Idle;
                self.ask_rx = None;
            }
            return Ok(());
        }
        // The constellation overlay swallows input; 'g' or Esc closes it.
        if self.show_graph {
            if key == 'g' || key == '\x1b' {
                self.show_graph = false;
            }
            return Ok(());
        }
        self.audio_msg = None;

        // ── 星火接线 sub-interaction — blocks grading until you resolve the recall ──
        let strike = self.current.as_ref().map(|c| c.strike).unwrap_or(Strike::Idle);
        match (strike, key) {
            (Strike::Prompt, ' ') => {
                if let Some(c) = &mut self.current {
                    c.strike = Strike::Flipped; // flip the card — reveal the anchor
                }
                return Ok(());
            }
            (Strike::Flipped, 'y' | 'Y') => {
                self.grade_anchor(rs_fsrs::Rating::Good)?; // recalled → real refresh
                return Ok(());
            }
            (Strike::Flipped, 'n' | 'N') => {
                self.grade_anchor(rs_fsrs::Rating::Again)?; // blanked → honest lapse
                return Ok(());
            }
            (Strike::Prompt | Strike::Flipped, '\x1b') => {
                self.should_quit = true;
                return Ok(());
            }
            (Strike::Prompt | Strike::Flipped, _) => return Ok(()), // swallow the rest
            _ => {}
        }

        // Universal keys, in every context.
        match key {
            '\x1b' => {
                self.should_quit = true;
                return Ok(());
            }
            ' ' => {
                self.play_audio();
                return Ok(());
            }
            '\n' | '\r' => {
                if let Some(c) = &mut self.current {
                    if c.stage == Stage::Prompt {
                        c.stage = Stage::Revealed;
                        // A learned sibling exists → light the 星火接线 prompt.
                        if c.anchor.is_some() {
                            c.strike = Strike::Prompt;
                        }
                    }
                }
                return Ok(());
            }
            _ => {}
        }

        // Derive game: on a new word's prompt, type keys build your guess.
        let in_derive = matches!(
            self.current.as_ref().map(|c| (c.is_new, c.stage)),
            Some((true, Stage::Prompt))
        );
        if in_derive {
            match key {
                '\x08' | '\x7f' => {
                    self.input.pop();
                }
                c if !c.is_control() => self.input.push(c),
                _ => {}
            }
            return Ok(());
        }

        // Command keys (review prompt / revealed / done).
        let revealed = matches!(
            self.current.as_ref().map(|c| c.stage),
            Some(Stage::Revealed)
        );
        match key {
            'q' => self.should_quit = true,
            'a' if revealed => {
                // With a typed guess on a new word, 'a' critiques YOUR reasoning;
                // otherwise it's a generic confusable 辨析.
                let has_guess = self.current.as_ref().map(|c| c.is_new).unwrap_or(false)
                    && !self.input.is_empty();
                if has_guess {
                    self.evaluate_guess();
                } else {
                    self.ask_socratic();
                }
            }
            'w' if revealed => self.open_wiktionary(),
            'g' if revealed => self.open_graph(),
            g @ '1'..='4' if revealed => self.grade(g as u8 - b'0')?,
            _ => {}
        }
        Ok(())
    }

    /// Play the current word's pronunciation — only through the bound earphone.
    /// Cached clips play instantly; uncached ones synthesize on demand via the warm
    /// server on a worker thread (spinner shows), then play.
    fn play_audio(&mut self) {
        let Some(word) = self.current.as_ref().map(|c| c.entry.word.clone()) else {
            return;
        };
        if !self.gate.open {
            self.audio_msg = Some("耳机未连接 · 静默".to_string());
            return;
        }
        let path = self.tts.cache_path(&word);
        if path.exists() {
            self.play_cached(&word, &path);
            return;
        }
        // Lazy synth: fire on a worker thread so the UI stays live.
        if self.tts_pending.is_some() {
            return;
        }
        if !self.tts.models_present() {
            self.audio_msg = Some("发音模型未下载（见 README）".to_string());
            return;
        }
        let tts = self.tts.clone();
        let server = self.tts_server.clone();
        let (w, out) = (word.clone(), path.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut guard = server.lock().unwrap();
            if guard.is_none() {
                match TtsServer::start(&tts) {
                    Ok(s) => *guard = Some(s),
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                }
            }
            let res = guard
                .as_mut()
                .unwrap()
                .synth(&w, &out, &tts.voice, tts.speed)
                .map(|_| out.clone())
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.tts_rx = Some(rx);
        self.tts_pending = Some(word);
    }

    fn play_cached(&mut self, word: &str, path: &Path) {
        if self.ensure_player() {
            if let Some(p) = &self.player {
                match p.play_file(path) {
                    Ok(()) => self.audio_msg = Some(format!("♪ {word}")),
                    Err(_) => self.audio_msg = Some("播放失败".to_string()),
                }
            }
        } else {
            self.audio_msg = Some("无法打开耳机输出".to_string());
        }
    }

    pub fn is_animating(&self) -> bool {
        matches!(self.ask, Ask::Pending)
            || self.tts_pending.is_some()
            || self
                .strike_anim
                .map(|t| t.elapsed().as_millis() < STRIKE_ANIM_MS)
                .unwrap_or(false)
    }

    /// Progress 0.0..1.0 of the strike arc, or None when it's not firing.
    pub fn strike_progress(&self) -> Option<f64> {
        let t = self.strike_anim?;
        let p = t.elapsed().as_millis() as f64 / STRIKE_ANIM_MS as f64;
        (p <= 1.0).then_some(p)
    }

    /// Pick the best learned sibling to anchor a new word: a shared root, weighted by
    /// specificity (a rare root beats the -tion flood) × kind × the reactivation band
    /// — prefer a fading-but-recoverable sibling, so recalling it does honest double duty.
    fn best_anchor(&self, word: &str) -> Option<Anchor> {
        let now = Utc::now();
        let mut best: Option<(f64, Anchor)> = None;
        for c in self.deck.anchor_candidates(word).ok()? {
            let specificity = 1.0 / (1.0 + c.members.max(1) as f64).ln();
            let kind_w = if c.morpheme_id.ends_with('-') {
                0.5 // prefix
            } else if c.morpheme_id.starts_with('-') {
                0.35 // suffix
            } else {
                1.0 // root
            };
            let r = c.card.get_retrievability(now); // 0..1
            let reactivation = 1.0 - ((r - 0.75).abs() * 1.6).min(1.0); // peaks at 0.75
            let score = specificity * kind_w * (0.5 + 0.5 * reactivation);
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((
                    score,
                    Anchor {
                        word: c.word,
                        surface: c.surface,
                        gloss_zh: c.gloss_zh,
                        card: c.card,
                    },
                ));
            }
        }
        best.map(|(_, a)| a)
    }

    /// Grade the recalled anchor (the OLD word): a real FSRS review, because the
    /// retrieval was real. The node heals only when retrieved, never when displayed.
    fn grade_anchor(&mut self, rating: rs_fsrs::Rating) -> Result<()> {
        let Some(anchor) = self.current.as_ref().and_then(|c| c.anchor.clone()) else {
            return Ok(());
        };
        let info = self.scheduler.grade(anchor.card.clone(), rating, Utc::now());
        self.deck.save_card(&anchor.word, &info.card, true)?;
        self.deck.log_review(&anchor.word, rating, &info.review_log)?;
        if let Some(c) = &mut self.current {
            c.strike = Strike::Idle; // resolved — grading the new word is unblocked
        }
        if matches!(rating, rs_fsrs::Rating::Good) {
            self.strike_anim = Some(Instant::now()); // fire the arc
            self.audio_msg = Some(format!("✦ 接线成功  {}  +1 复习", anchor.word));
        } else {
            self.audio_msg = Some(format!("· 揭示  {}  (记一次待复习)", anchor.word));
        }
        Ok(())
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

    /// Fire a Socratic 辨析 request on a worker thread (non-blocking UI).
    fn ask_socratic(&mut self) {
        let Some(c) = self.current.as_ref() else {
            return;
        };
        if self.ds_key.is_empty() {
            self.ask = Ask::Failed("未配置 DeepSeek 密钥（tuna.toml）".to_string());
            return;
        }
        let word = c.entry.word.clone();
        let context = socratic_context(c);
        let (base, key, model) = (
            self.ds_base.clone(),
            self.ds_key.clone(),
            self.ds_chat_model.clone(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let client = DeepSeek::new(base, key);
            let res = crate::llm::socratic::socratic(&client, &model, &word, &context)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.ask_rx = Some(rx);
        self.ask = Ask::Pending;
    }

    /// Open the current word's Wiktionary etymology in the browser — the citation
    /// behind every root is one keystroke away. Honesty as a keypress.
    fn open_wiktionary(&mut self) {
        let Some(word) = self.current.as_ref().map(|c| c.entry.word.clone()) else {
            return;
        };
        let url = format!("https://en.wiktionary.org/wiki/{word}#English");
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(&url).spawn();
        self.audio_msg = Some(format!("↗ Wiktionary · {word}"));
    }

    /// Open the constellation — the current word's root-family, the words already lit
    /// in your galaxy and the frontier that's one root away. Nothing here is invented:
    /// every edge is a shared morpheme node, every glow is real FSRS stability.
    fn open_graph(&mut self) {
        let Some(word) = self.current.as_ref().map(|c| c.entry.word.clone()) else {
            return;
        };
        self.graph = self.deck.constellation(&word).unwrap_or_default();
        if self.graph.is_empty() {
            self.audio_msg = Some("这个词没有共享词根的邻居".to_string());
            return;
        }
        self.show_graph = true;
    }

    /// Send the learner's OWN derivation guess to DeepSeek for a Socratic critique of
    /// his reasoning — the guess becomes a live channel, not a dead echo.
    fn evaluate_guess(&mut self) {
        let Some(c) = self.current.as_ref() else {
            return;
        };
        if self.ds_key.is_empty() {
            self.ask = Ask::Failed("未配置 DeepSeek 密钥（~/.tuna/config.toml）".to_string());
            return;
        }
        let word = c.entry.word.clone();
        let morphemes = c
            .enrichment
            .as_ref()
            .map(|en| {
                en.morphemes
                    .iter()
                    .map(|m| format!("{}({})", m.unit, m.meaning_zh))
                    .collect::<Vec<_>>()
                    .join(" + ")
            })
            .unwrap_or_default();
        let guess = self.input.clone();
        let (base, key, model) = (
            self.ds_base.clone(),
            self.ds_key.clone(),
            self.ds_chat_model.clone(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let client = DeepSeek::new(base, key);
            let res = crate::llm::socratic::evaluate_guess(&client, &model, &word, &morphemes, &guess)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.ask_rx = Some(rx);
        self.ask = Ask::Pending;
    }

    /// Drain any completed background work (Socratic answer, on-demand synth).
    pub fn poll_async(&mut self) {
        if let Some(rx) = &self.ask_rx {
            if let Ok(res) = rx.try_recv() {
                if matches!(self.ask, Ask::Pending) {
                    self.ask = match res {
                        Ok(t) => Ask::Answer(t),
                        Err(e) => Ask::Failed(e),
                    };
                }
                self.ask_rx = None;
            }
        }
        if let Some(rx) = &self.tts_rx {
            if let Ok(res) = rx.try_recv() {
                let word = self.tts_pending.take().unwrap_or_default();
                self.tts_rx = None;
                match res {
                    Ok(path) => {
                        if self.gate.open {
                            self.play_cached(&word, &path);
                        }
                    }
                    Err(e) => {
                        let first = e.lines().next().unwrap_or("synth failed").to_string();
                        self.audio_msg = Some(format!("合成失败: {first}"));
                    }
                }
            }
        }
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

/// Build context for a Socratic request from the card's enrichment (confusables +
/// near-synonyms + gloss) so the model contrasts the right neighbours.
fn socratic_context(c: &CardView) -> String {
    let mut s = String::new();
    if let Some(en) = &c.enrichment {
        if !en.gloss_zh.is_empty() {
            s.push_str(&format!("词义: {}\n", en.gloss_zh));
        }
        let neighbours: Vec<String> = en
            .graph_edges
            .iter()
            .filter(|e| e.relation == "confusable" || e.relation == "synonym")
            .map(|e| e.target.clone())
            .collect();
        if !neighbours.is_empty() {
            s.push_str(&format!("易混/近义: {}\n", neighbours.join(", ")));
        }
    } else if let Some(t) = c.entry.translation.lines().next() {
        s.push_str(&format!("词义: {}\n", t.trim()));
    }
    s
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
