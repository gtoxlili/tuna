//! Study session state and logic. A synchronous model: the render loop drives it,
//! keys mutate it, and the earphone gate is re-polled on a ~1s cadence. No async
//! runtime needed for the review loop — background work (LLM, audio) arrives later
//! over channels, keeping this core simple and robust for daily use.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::audio::player::{self, RoutedPlayer};
use crate::audio::probe;
use crate::audio::tts::{self, SynthSession, TtsConfig, TtsEngineKind, from_kind};
use crate::config::Config;
use crate::data::deck::{DeckCard, DictEntry, MorphemeGroup};
use crate::data::scheduler::rating_from_u8;
use crate::data::{Deck, Scheduler};
use crate::llm::DeepSeek;
use crate::llm::enrich::Enrichment;
use crate::paths;

use super::cmdmenu::CommandMenu;
use super::settings::Settings;

/// Introductions per session — the comfortable 2028 pace (leaves room for reviews).
const NEW_PER_SESSION: usize = 15;
const REVIEW_CAP: usize = 300;
const GATE_POLL: Duration = Duration::from_millis(1000);
/// Strike arc duration. Was 900ms — too long: non-reduced-motion users saw the arc
/// for 900ms while reduced-motion users saw siblings immediately, so the two
/// populations read different content for the first 900ms. 400ms keeps the
/// "lighting up" beat without diverging from the reduced view.
const STRIKE_ANIM_MS: u128 = 400;
/// Duration of the post-grade border flash (Again/Hard/Good/Easy each their own color).
/// 250ms is enough to register as tactile weight without lingering past the next card.
const GRADE_FLASH_MS: u128 = 250;
/// Card slide-in: a short fade so a new card "arrives" instead of jump-cutting.
const CARD_SLIDE_MS: u128 = 150;
/// Morpheme stagger step on reveal — each cell lights up this many ms after the
/// previous one, preserving the "taking it apart" feel of the derivation.
pub const MORPHEME_STAGGER_MS: u128 = 60;
/// Per-cell fade-in duration (starts at index × STAGGER). 120ms is one eye-fixation;
/// shorter feels like a flicker, longer drags the reveal past the user's read.
pub const MORPHEME_CELL_FADE_MS: u128 = 120;
/// Window for the two-press Esc-to-quit confirmation (first Esc primes, second quits).
const ESC_CONFIRM_MS: Duration = Duration::from_secs(2);

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

/// A speakable item on a revealed card — the word itself or one of its example
/// sentences. Arrow keys cycle the `speak_cursor` through these; Space speaks the
/// selected one (default: the word).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Speakable {
    Word,
    Example(usize),
}

/// Severity of a transient toast — drives color and time-to-live. `Error` is sticky
/// (needs a keypress to dismiss); `Info` fades in 3s; `Warn` in 5s.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

/// A transient one-line message (replaces the old `audio_msg: Option<String>`).
/// `born` lets `poll_async` expire it; `level` drives color + TTL.
#[derive(Clone, Debug)]
pub struct ToastMsg {
    pub text: String,
    pub level: ToastLevel,
    pub born: Instant,
}

impl ToastMsg {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: ToastLevel::Info,
            born: Instant::now(),
        }
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: ToastLevel::Warn,
            born: Instant::now(),
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: ToastLevel::Error,
            born: Instant::now(),
        }
    }
    fn ttl(&self) -> Option<Duration> {
        match self.level {
            ToastLevel::Info => Some(Duration::from_secs(3)),
            ToastLevel::Warn => Some(Duration::from_secs(5)),
            ToastLevel::Error => None,
        }
    }
    fn expired(&self) -> bool {
        self.ttl().is_some_and(|ttl| self.born.elapsed() >= ttl)
    }
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

/// One turn in the derivation chat: the learner's message or the LLM's reply.
#[derive(Clone)]
pub struct ChatTurn {
    pub is_user: bool,
    pub text: String,
}

/// State of the derivation chat overlay (Phase A only — "拆" with LLM guidance).
/// Closed = not active; Open = user can type; Pending = message sent, awaiting LLM.
#[derive(PartialEq)]
pub enum DeriveState {
    Closed,
    Open,
    Pending,
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
    /// Which speakable item (word / example) is highlighted for Space-to-speak.
    pub speak_cursor: usize,
}

pub struct App {
    pub deck: Deck,
    pub scheduler: Scheduler,
    pub needle: String,
    pub tts: TtsConfig,
    /// Playback stream, held open only while the earphone is present.
    player: Option<RoutedPlayer>,
    /// Warm synth session (sherpa OfflineTts), started lazily on first on-demand synth.
    tts_server: Arc<Mutex<Option<Box<dyn SynthSession>>>>,
    tts_rx: Option<std::sync::mpsc::Receiver<std::result::Result<PathBuf, String>>>,
    /// The word currently being synthesized (drives the spinner).
    pub tts_pending: Option<String>,
    /// Transient one-line toast (replaces `audio_msg`): drives color + TTL by level.
    pub toast: Option<ToastMsg>,
    /// Accessibility: skip all animation when true (read from config [a11y]).
    pub reduced_motion: bool,
    /// The learner's typed guess in the derive game (Phase A).
    pub input: String,
    /// Animation clock (spinners advance off this).
    pub anim: Instant,
    /// Card slide-in clock, set each time a new card loads. Drives a 150ms fade so
    /// the card "arrives" rather than jump-cutting. None / past-window → no fade.
    pub card_slide: Option<Instant>,
    /// Reveal clock, set when a Prompt card flips to Revealed. Drives the morpheme
    /// stagger (each cell lights up 60ms after the previous). None / past-window →
    /// all cells render solid.
    pub reveal_anim: Option<Instant>,
    /// Set when the 星火接线 arc fires (successful recall) — drives a brief animation.
    pub strike_anim: Option<Instant>,
    /// Set right after a grade — drives a brief border-color flash on the card
    /// (Again=red / Good=green / Easy=amber) so the act of grading has tactile weight.
    pub grade_flash: Option<(rs_fsrs::Rating, Instant)>,
    pub queue: Vec<DeckCard>,
    pub pos: usize,
    pub current: Option<CardView>,
    pub gate: GateStatus,
    last_gate_poll: Instant,
    pub session_new: u32,
    pub session_reviews: u32,
    pub session_total: usize,
    pub should_quit: bool,
    /// Esc-at-base confirmation: first Esc sets this to now + a "press again to quit"
    /// toast; a second Esc within the window actually quits. Stops accidental exits
    /// dropping unsaved review state.
    pub esc_confirm: Option<Instant>,
    /// Whether the constellation (root-family map) overlay is open.
    pub show_graph: bool,
    /// Whether the `?` help overlay is open (context-sensitive key reference).
    pub show_help: bool,
    /// Vertical scroll offset for the Socratic `Ask::Answer` popup — arrow keys scroll.
    pub ask_scroll: usize,
    /// The current word's root-family, computed when the overlay opens.
    pub graph: Vec<MorphemeGroup>,
    /// Flattened cursor into the constellation overlay's members (arrow-key nav).
    pub graph_cursor: usize,
    /// Runtime TTS engine switcher overlay.
    pub settings: Settings,
    /// Tab command menu — the "software化" primary command surface.
    pub cmdmenu: CommandMenu,
    /// Undo snapshot: the card + position before the most recent grade, with a
    /// timestamp. One-step undo within a 3s window (Anki AJT style); past the
    /// window the grade is final. Multi-step undo would let the displayed card
    /// flow diverge from FSRS's review history — breaking parameter trust.
    undo_snap: Option<(DeckCard, Instant)>,
    // Socratic 辨析 (live DeepSeek on a worker thread)
    pub ask: Ask,
    ask_rx: Option<std::sync::mpsc::Receiver<(u64, std::result::Result<String, String>)>>,
    /// Generation counter for ask requests — each new request increments this.
    /// Worker threads capture the gen at spawn time and send it back; poll_async
    /// discards results whose gen != current, so a cancelled-then-reopened ask
    /// doesn't land a stale answer from the first (still-billing) worker.
    ask_gen: u64,
    /// Multi-turn derivation chat state (Phase A "拆" with LLM guidance).
    pub derive: DeriveState,
    /// Conversation history for the current card's derivation chat.
    pub derive_turns: Vec<ChatTurn>,
    /// Receiver for derivation chat LLM responses (separate from ask_rx).
    derive_rx: Option<std::sync::mpsc::Receiver<std::result::Result<String, String>>>,
    /// Scroll offset for the derivation chat popup.
    pub derive_scroll: usize,
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
            toast: None,
            reduced_motion: cfg.a11y.reduced_motion,
            input: String::new(),
            anim: Instant::now(),
            card_slide: None,
            reveal_anim: None,
            strike_anim: None,
            grade_flash: None,
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
            esc_confirm: None,
            show_graph: false,
            show_help: false,
            ask_scroll: 0,
            graph: Vec::new(),
            graph_cursor: 0,
            settings: Settings::default(),
            cmdmenu: CommandMenu::default(),
            undo_snap: None,
            ask: Ask::Idle,
            ask_rx: None,
            ask_gen: 0,
            derive: DeriveState::Closed,
            derive_turns: Vec::new(),
            derive_rx: None,
            derive_scroll: 0,
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
        // Drop any in-flight TTS — it was requested for the previous card and must not
        // play on the new one. The worker thread's `tx.send` will fail silently on the
        // dropped receiver; the synthesized WAV is still cached for next time.
        self.tts_pending = None;
        self.tts_rx = None;
        // Reset per-card transient state — without this, strike_anim keeps the arc
        // firing from the new card's anchor on the old clock, and a half-open
        // ask/graph overlay bleeds onto the new card. One clean slate per card.
        // NOTE: grade_flash is NOT cleared here — it's a cross-card transient by
        // design: grade() sets it, then advances to the next card, and the wash
        // carries over to tint the new card's first ~250ms as feedback for which
        // rating key was pressed. It self-expires in poll_async (D6) and is cleared
        // explicitly on undo_grade. Clearing it here would make the flash never show.
        self.strike_anim = None;
        self.ask = Ask::Idle;
        self.ask_rx = None;
        self.ask_scroll = 0;
        self.show_graph = false;
        self.graph_cursor = 0;
        self.derive = DeriveState::Closed;
        self.derive_turns.clear();
        self.derive_rx = None;
        self.derive_scroll = 0;
        // A new card is loading — prime the slide-in so it fades up rather than
        // jump-cutting. reset_motion users get None (no fade, instant).
        self.reveal_anim = None;
        self.card_slide = if self.reduced_motion {
            None
        } else {
            Some(Instant::now())
        };
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
                    speak_cursor: 0,
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
                speak_cursor: 0,
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

    /// Open the settings overlay with the cursor pre-positioned on the current engine.
    /// Without this, a Cancel-and-reopen lands the cursor where you last left it, which
    /// reads as "the highlighted engine is the current one" even when it isn't.
    fn open_settings(&mut self) {
        let kinds = TtsEngineKind::all();
        self.settings.cursor = kinds.iter().position(|k| *k == self.tts.kind).unwrap_or(0);
        self.settings.open = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        // `?` is meta — always available, even with overlays open. It's the discovery
        // backstop: if the learner forgets which key does what inside any overlay, '?'
        // surfaces the context-sensitive reference without first dismissing the overlay.
        // Only Esc/? dismiss an open help; other keys fall through to the underlying
        // overlay so the learner can act without a double-press (e.g. 's' over settings
        // closes help then toggles settings in one stroke).
        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.show_help = false;
                return Ok(());
            }
            self.show_help = false;
            // Fall through — the key reaches the overlay/base handler below.
        } else if matches!(key.code, KeyCode::Char('?')) {
            self.show_help = true;
            return Ok(());
        }
        // The settings overlay has the highest priority — it swallows all input.
        if self.settings.open {
            self.on_settings_key(key)?;
            return Ok(());
        }
        // The Socratic popup swallows input; 'a' or Esc closes it. Arrow keys scroll
        // the answer when it overflows the popup (DeepSeek 输出经常超 30 行).
        if !matches!(self.ask, Ask::Idle) {
            if matches!(key.code, KeyCode::Char('a') | KeyCode::Esc) {
                self.ask = Ask::Idle;
                self.ask_rx = None;
                self.ask_scroll = 0;
                return Ok(());
            }
            if let Ask::Answer(text) = &self.ask {
                match key.code {
                    KeyCode::Down | KeyCode::Right => {
                        // Clamp so the user can't scroll past the content into blank.
                        // The popup shows roughly 10 lines after borders/padding; the
                        // raw line count overestimates visible lines (wrapping adds
                        // more), so this is a safe upper bound.
                        let max = text.lines().count().saturating_sub(10);
                        self.ask_scroll = (self.ask_scroll + 1).min(max);
                        return Ok(());
                    }
                    KeyCode::Up | KeyCode::Left => {
                        self.ask_scroll = self.ask_scroll.saturating_sub(1);
                        return Ok(());
                    }
                    _ => {}
                }
            }
            // The overlay swallows keys that would otherwise grade or open other
            // overlays. Surface the swallow — a silent return makes the user think
            // the grade registered when it didn't.
            if matches!(
                key.code,
                KeyCode::Char('1'..='4' | 'h' | 'j' | 'k' | 'l' | 'w' | 's' | 'g' | ' ')
                    | KeyCode::Enter
            ) {
                self.toast = Some(ToastMsg::info("先 a/Esc 关闭辨析"));
            }
            return Ok(());
        }
        // The constellation overlay swallows input; arrow nav + Space speak + g/Esc close.
        if self.show_graph {
            return self.on_graph_key(key);
        }
        // The command menu: Tab opens it (anywhere except deeper overlays), ↑↓ moves,
        // Enter fires the selected command, letters (a/w/g/s/?) fire directly, Esc/Tab
        // close. The menu sits below settings/ask/graph — those swallow input first —
        // but above help and the base card, so it's reachable from Prompt, Revealed,
        // and done alike (settings is a runtime concern, not card-stage-bound).
        if self.cmdmenu.open {
            return self.on_cmdmenu_key(key);
        }
        if matches!(key.code, KeyCode::Tab) {
            self.cmdmenu.open = true;
            self.cmdmenu.cursor = 0;
            return Ok(());
        }
        // The derivation chat overlay — multi-turn LLM conversation for Phase A "拆".
        // Sits below cmdmenu (Tab still works) but above the base card. When open or
        // pending, printable chars build the chat input; Enter sends, Esc closes.
        if self.derive != DeriveState::Closed {
            return self.on_derive_key(key);
        }
        // Clear any non-sticky toast on the next keypress — Error toasts stay until
        // an explicit Esc/Space so the user can read a failure message even if they
        // reflexively hit a no-op key (like 'z' at base) right after it appears.
        if !matches!(
            self.toast.as_ref().map(|t| t.level),
            Some(ToastLevel::Error)
        ) {
            self.toast = None;
        }
        // A non-Esc key cancels the Esc-to-quit priming. Esc itself is excluded so the
        // two-press flow works: the first Esc primes at line ~541, the second Esc must
        // still find that prime here to quit. Clearing unconditionally would re-prime on
        // every press and trap the user mid-session.
        if !matches!(key.code, KeyCode::Esc) {
            self.esc_confirm = None;
        }

        // ── 星火接线 sub-interaction — blocks grading until you resolve the recall ──
        let strike = self
            .current
            .as_ref()
            .map(|c| c.strike)
            .unwrap_or(Strike::Idle);
        match (strike, key.code) {
            (Strike::Prompt, KeyCode::Char(' ')) => {
                if let Some(c) = &mut self.current {
                    c.strike = Strike::Flipped; // flip the card — reveal the anchor
                }
                return Ok(());
            }
            (Strike::Flipped, KeyCode::Char('y' | 'Y')) => {
                self.grade_anchor(rs_fsrs::Rating::Good)?; // recalled → real refresh
                return Ok(());
            }
            (Strike::Flipped, KeyCode::Char('n' | 'N')) => {
                self.grade_anchor(rs_fsrs::Rating::Again)?; // blanked → honest lapse
                return Ok(());
            }
            (Strike::Prompt | Strike::Flipped, KeyCode::Esc) => {
                // Esc in strike = "skip this recall check", not "kill the program".
                // Drop the anchor without grading it and fall through to normal review.
                if let Some(c) = &mut self.current {
                    c.strike = Strike::Idle;
                }
                return Ok(());
            }
            (Strike::Prompt | Strike::Flipped, _) => return Ok(()), // swallow the rest
            _ => {}
        }

        // Universal keys, in every context.
        match key.code {
            KeyCode::Esc => {
                // Single Esc quits when the session is done — there's no review
                // state left to lose, so a confirmation gate there is pure
                // friction. At base (mid-session) the two-press gate still applies.
                if self.done() {
                    self.should_quit = true;
                    return Ok(());
                }
                if let Some(t) = self.esc_confirm {
                    if t.elapsed() < ESC_CONFIRM_MS {
                        self.should_quit = true;
                        return Ok(());
                    }
                }
                self.esc_confirm = Some(Instant::now());
                self.toast = Some(ToastMsg::warn("再按 Esc 退出"));
                return Ok(());
            }
            KeyCode::Char(' ') => {
                self.play_speakable();
                return Ok(());
            }
            KeyCode::Enter => {
                if let Some(c) = &mut self.current {
                    if c.stage == Stage::Prompt {
                        c.stage = Stage::Revealed;
                        // A learned sibling exists → light the 星火接线 prompt.
                        if c.anchor.is_some() {
                            c.strike = Strike::Prompt;
                        }
                        // Prime the morpheme stagger so the derivation "unfolds"
                        // cell-by-cell instead of dumping all at once. Reduced-motion
                        // users get None → cells render solid immediately.
                        self.reveal_anim = if self.reduced_motion {
                            None
                        } else {
                            Some(Instant::now())
                        };
                    }
                }
                return Ok(());
            }
            _ => {}
        }

        let revealed = matches!(
            self.current.as_ref().map(|c| c.stage),
            Some(Stage::Revealed)
        );

        // Arrow keys: ↑↓ cycle the speakable cursor (word / examples) when revealed.
        // ←→ is a silent no-op here — 4 keys doing the same thing is semantic noise,
        // and reserving ←→ for a future horizontal use is cleaner than mirroring ↑↓.
        if revealed {
            match key.code {
                KeyCode::Up => {
                    self.move_speak_cursor(-1);
                    return Ok(());
                }
                KeyCode::Down => {
                    self.move_speak_cursor(1);
                    return Ok(());
                }
                _ => {}
            }
        }

        // Command keys. `a` on a new word's Prompt opens the derivation chat (拆
        // with LLM guidance); on Revealed or review cards it opens generic 辨析.
        // The other commands (w/g/s/grade) require revealed.
        let is_new_prompt = matches!(
            self.current.as_ref().map(|c| (c.is_new, c.stage)),
            Some((true, Stage::Prompt))
        );
        let done = self.done();
        match key.code {
            KeyCode::Char('a') if is_new_prompt => self.start_derive_chat(),
            KeyCode::Char('a') if revealed || done => self.ask_socratic(),
            KeyCode::Char('w') if revealed => self.open_wiktionary(),
            KeyCode::Char('g') if revealed => self.open_graph(),
            // Settings is a runtime concern — reachable even after the session ends
            // (done state), so the learner can switch TTS engines without restarting.
            KeyCode::Char('s') if revealed || done => self.open_settings(),
            KeyCode::Char(c @ '1'..='4') if revealed => self.grade(c as u8 - b'0')?,
            // hjkl mirror the 1-4 grade keys (home-row hand position, Anki AJT style).
            // Explicit match — not a 'h'..='l' range, which would swallow 'i'.
            KeyCode::Char(c) if revealed && matches!(c, 'h' | 'j' | 'k' | 'l') => {
                self.grade(match c {
                    'h' => 1,
                    'j' => 2,
                    'k' => 3,
                    'l' => 4,
                    _ => 0,
                })?;
            }
            // Undo the last grade (3s window). Not gated on `revealed` because the
            // new card after grading may still be in Prompt — undo must reach back
            // across the card boundary regardless of the current card's stage.
            KeyCode::Char('u') => return self.undo_grade(),
            _ => {}
        }
        Ok(())
    }

    /// Handle keys while the Tab command menu is open. ↑↓ moves the cursor (skipping
    /// disabled rows), Enter fires the selected command, letters (a/w/g/s/?) fire
    /// directly as expert-mode shortcuts, and Esc/Tab closes. The menu is the primary
    /// "software化" surface — a learner who knows nothing about shortcuts can reach
    /// every command via Tab → ↑↓ → Enter.
    fn on_cmdmenu_key(&mut self, key: KeyEvent) -> Result<()> {
        let items = self.cmdmenu.items(self);
        match key.code {
            KeyCode::Tab | KeyCode::Esc => {
                self.cmdmenu.open = false;
            }
            KeyCode::Up => self.cmdmenu.move_cursor(-1, &items),
            KeyCode::Down => self.cmdmenu.move_cursor(1, &items),
            KeyCode::Enter => {
                if let Some(it) = items.get(self.cmdmenu.cursor) {
                    if it.enabled {
                        let label = it.label;
                        self.cmdmenu.open = false;
                        return self.fire_command(label);
                    }
                }
            }
            // Direct letter shortcuts — expert mode. They fire the command and close
            // the menu in one stroke, so power users aren't slowed by the menu.
            KeyCode::Char('a') => {
                self.cmdmenu.open = false;
                return self.fire_command("辨析");
            }
            KeyCode::Char('w') => {
                self.cmdmenu.open = false;
                return self.fire_command("词源");
            }
            KeyCode::Char('g') => {
                self.cmdmenu.open = false;
                return self.fire_command("星座");
            }
            KeyCode::Char('s') => {
                self.cmdmenu.open = false;
                return self.fire_command("设置");
            }
            KeyCode::Char('?') => {
                self.cmdmenu.open = false;
                return self.fire_command("帮助");
            }
            KeyCode::Char('u') => {
                if items.iter().any(|it| it.label == "撤销评分" && it.enabled) {
                    self.cmdmenu.open = false;
                    return self.fire_command("撤销评分");
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Fire a command by its label string. Shared between Enter-on-selected and
    /// direct-letter shortcuts so the two paths never drift.
    fn fire_command(&mut self, label: &str) -> Result<()> {
        match label {
            "辨析" => {
                let is_new_prompt = matches!(
                    self.current.as_ref().map(|c| (c.is_new, c.stage)),
                    Some((true, Stage::Prompt))
                );
                if is_new_prompt {
                    self.start_derive_chat();
                } else {
                    self.ask_socratic();
                }
            }
            "词源" => self.open_wiktionary(),
            "星座" => self.open_graph(),
            "设置" => self.open_settings(),
            "撤销评分" => return self.undo_grade(),
            "帮助" => self.show_help = true,
            _ => {}
        }
        Ok(())
    }

    /// The speakable items on the current revealed card: the word itself, plus any
    /// example sentences from enrichment. Empty if there's no current card.
    fn speakables(&self) -> Vec<Speakable> {
        let Some(c) = &self.current else {
            return Vec::new();
        };
        let mut items = vec![Speakable::Word];
        if let Some(en) = &c.enrichment {
            for (i, ex) in en.examples.iter().enumerate() {
                if !ex.en.is_empty() {
                    items.push(Speakable::Example(i));
                }
            }
        }
        // Cap at word + 2 examples to match the display cap in derivation_reveal.
        items.truncate(3);
        items
    }

    fn current_speakable(&self) -> Option<Speakable> {
        let cursor = self.current.as_ref()?.speak_cursor;
        self.speakables().get(cursor).copied()
    }

    /// Move the speakable cursor by `delta` (wraps around). Clamps to the list bounds.
    fn move_speak_cursor(&mut self, delta: i32) {
        let n = self.speakables().len();
        if n <= 1 {
            return;
        }
        let Some(c) = &mut self.current else {
            return;
        };
        let mut idx = c.speak_cursor as i32 + delta;
        while idx < 0 {
            idx += n as i32;
        }
        c.speak_cursor = (idx as usize) % n;
    }

    /// Play the currently-highlighted speakable item (word or example sentence).
    fn play_speakable(&mut self) {
        let Some(c) = &self.current else {
            return;
        };
        let (text, label) = match self.current_speakable() {
            Some(Speakable::Word) => (c.entry.word.clone(), c.entry.word.clone()),
            Some(Speakable::Example(i)) => {
                let Some(en) = &c.enrichment else { return };
                let Some(ex) = en.examples.get(i) else { return };
                if ex.en.is_empty() {
                    return;
                }
                (ex.en.clone(), "例句".to_string())
            }
            None => return,
        };
        self.play_audio(&text, &label);
    }

    /// Synthesize + play `text` through the bound earphone. Cached clips play
    /// instantly; uncached ones synthesize on a worker thread (spinner shows the
    /// `label`), then play. `label` is the short display string for the spinner.
    fn play_audio(&mut self, text: &str, label: &str) {
        if !self.gate.open {
            self.toast = Some(ToastMsg::warn("耳机未连接 · 静默"));
            return;
        }
        let path = self.tts.cache_path(text);
        if path.exists() {
            self.play_cached(label, &path);
            return;
        }
        // Lazy synth: fire on a worker thread so the UI stays live.
        if self.tts_pending.is_some() {
            return;
        }
        if !self.tts.models_present() {
            self.toast = Some(ToastMsg::error(
                "发音模型未下载（运行 tuna setup 下载，或按 s 打开设置）",
            ));
            return;
        }
        let tts = self.tts.clone();
        let server = self.tts_server.clone();
        let (t, out) = (text.to_string(), path.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Recover from a poisoned mutex (a prior worker panicked while holding
            // the lock). The inner Option is still valid; we just take it as-is.
            // Without this, a single worker panic permanently kills all TTS.
            let mut guard = server.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                match tts::start_session(&tts) {
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
                .synth(&t, &out, &tts.voice, tts.speed)
                .map(|_| out.clone())
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        self.tts_rx = Some(rx);
        self.tts_pending = Some(label.to_string());
    }

    /// Handle keys while the settings overlay is open.
    fn on_settings_key(&mut self, key: KeyEvent) -> Result<()> {
        let n = TtsEngineKind::all().len();
        match key.code {
            KeyCode::Up => {
                self.settings.cursor = (self.settings.cursor + n - 1) % n;
            }
            KeyCode::Down => {
                self.settings.cursor = (self.settings.cursor + 1) % n;
            }
            KeyCode::Enter => {
                let kind = TtsEngineKind::all()[self.settings.cursor];
                let eng = from_kind(kind);
                let dir = paths::engine_dir(kind);
                if eng.models_present(&dir) {
                    let voice = eng.default_voice().id;
                    match crate::config::update_tts(kind.id(), &voice) {
                        Ok(()) => {
                            // Reload config + drop the warm session so next synth uses
                            // the new engine. If the reload fails (brittle TOML
                            // write/read), surface it — silently keeping the old engine
                            // while the toast says "切换成功" would mislead the user.
                            match Config::load() {
                                Ok(cfg) => self.tts = cfg.tts_engine(),
                                Err(e) => {
                                    self.toast = Some(ToastMsg::error(format!(
                                        "配置写入成功但重载失败: {e}"
                                    )));
                                    return Ok(());
                                }
                            }
                            self.tts_server = Arc::new(Mutex::new(None));
                            // Drop the in-flight synth's receiver so the old worker's
                            // result is discarded. The worker itself runs to completion
                            // (sherpa's C++ call can't be interrupted), but its output
                            // lands in the old engine's cache path and is never served.
                            self.tts_pending = None;
                            self.tts_rx = None;
                            self.settings.open = false;
                            self.toast = Some(ToastMsg::info(format!("✓ 切换到 {}", kind.id())));
                        }
                        Err(e) => {
                            self.toast = Some(ToastMsg::error(format!("切换失败: {e}")));
                        }
                    }
                } else {
                    self.toast = Some(ToastMsg::error(format!(
                        "{} 未下载，运行 tuna setup 下载",
                        kind.id()
                    )));
                }
            }
            KeyCode::Esc | KeyCode::Char('s') => {
                self.settings.open = false;
            }
            _ => {
                if matches!(
                    key.code,
                    KeyCode::Char('1'..='4' | 'h' | 'j' | 'k' | 'l' | 'a' | 'w' | 'g' | 'u')
                        | KeyCode::Enter
                        | KeyCode::Tab
                ) {
                    self.toast = Some(ToastMsg::info("先 s/Esc 关闭设置"));
                }
            }
        }
        Ok(())
    }

    /// Handle keys while the constellation overlay is open.
    fn on_graph_key(&mut self, key: KeyEvent) -> Result<()> {
        let members = self.graph_members();
        let n = members.len();
        match key.code {
            KeyCode::Up => {
                if n > 0 {
                    self.graph_cursor = (self.graph_cursor + n - 1) % n;
                }
            }
            KeyCode::Down => {
                if n > 0 {
                    self.graph_cursor = (self.graph_cursor + 1) % n;
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(word) = members.get(self.graph_cursor).cloned() {
                    self.play_audio(&word, &word);
                }
            }
            KeyCode::Char('g') | KeyCode::Esc => {
                self.show_graph = false;
            }
            _ => {
                // Surface swallowed grade/command keys — without this the user can't
                // tell whether '3' graded the word or vanished into the overlay.
                if matches!(
                    key.code,
                    KeyCode::Char('1'..='4' | 'h' | 'j' | 'k' | 'l' | 'a' | 'w' | 's')
                        | KeyCode::Enter
                ) {
                    self.toast = Some(ToastMsg::info("先 g/Esc 关闭星座"));
                }
            }
        }
        Ok(())
    }

    /// Flatten the constellation's members into a flat word list (for cursor indexing).
    /// Mirrors `render_constellation`'s per-group sort + cap so arrow-key navigation
    /// follows the visual order exactly — the Nth word here is the Nth word drawn.
    /// Both sides call the shared `sort_members` + `GRAPH_MEMBER_CAP`, so they can't drift.
    fn graph_members(&self) -> Vec<String> {
        let word = self
            .current
            .as_ref()
            .map(|c| c.entry.word.as_str())
            .unwrap_or("");
        self.graph
            .iter()
            .flat_map(|g| {
                let mut ms = g.members.clone();
                crate::data::deck::sort_members(&mut ms, word);
                ms.into_iter()
                    .take(crate::data::deck::GRAPH_MEMBER_CAP)
                    .map(|m| m.word)
            })
            .collect()
    }

    fn play_cached(&mut self, word: &str, path: &Path) {
        if self.ensure_player() {
            if let Some(p) = &self.player {
                match p.play_file(path) {
                    Ok(()) => self.toast = Some(ToastMsg::info(format!("♪ {word}"))),
                    Err(_) => self.toast = Some(ToastMsg::error("播放失败")),
                }
            }
        } else {
            self.toast = Some(ToastMsg::error("无法打开耳机输出"));
        }
    }

    pub fn is_animating(&self) -> bool {
        matches!(self.ask, Ask::Pending)
            || self.derive == DeriveState::Pending
            || self.tts_pending.is_some()
            || self
                .strike_anim
                .map(|t| t.elapsed().as_millis() < STRIKE_ANIM_MS)
                .unwrap_or(false)
            || self
                .grade_flash
                .map(|(_, t)| t.elapsed().as_millis() < GRADE_FLASH_MS)
                .unwrap_or(false)
            || self
                .card_slide
                .map(|t| t.elapsed().as_millis() < CARD_SLIDE_MS)
                .unwrap_or(false)
            || self
                .reveal_anim
                .map(|t| {
                    let n = self.morpheme_count() as u128;
                    t.elapsed().as_millis() < n * MORPHEME_STAGGER_MS + MORPHEME_CELL_FADE_MS
                })
                .unwrap_or(false)
    }

    /// Progress 0.0..1.0 of the card slide-in fade, or None once settled.
    pub fn card_slide_progress(&self) -> Option<f64> {
        let t = self.card_slide?;
        let p = t.elapsed().as_millis() as f64 / CARD_SLIDE_MS as f64;
        (p <= 1.0).then_some(p)
    }

    /// Number of morpheme cells on the current card (drives the reveal-animation
    /// window). Returns 0 when there's no enrichment — nothing to stagger.
    fn morpheme_count(&self) -> usize {
        self.current
            .as_ref()
            .and_then(|c| c.enrichment.as_ref())
            .map(|e| e.morphemes.len())
            .unwrap_or(0)
    }

    /// Elapsed ms since the Prompt→Revealed flip, for morpheme stagger timing.
    /// None means "render all cells solid" (reduced-motion, or past the window).
    pub fn reveal_elapsed_ms(&self) -> Option<u128> {
        let t = self.reveal_anim?;
        let ms = t.elapsed().as_millis();
        let n = self.morpheme_count() as u128;
        let window = n * MORPHEME_STAGGER_MS + MORPHEME_CELL_FADE_MS;
        (ms < window).then_some(ms)
    }

    /// Progress 0.0..1.0 of the strike arc, or None when it's not firing.
    pub fn strike_progress(&self) -> Option<f64> {
        let t = self.strike_anim?;
        let p = t.elapsed().as_millis() as f64 / STRIKE_ANIM_MS as f64;
        (p <= 1.0).then_some(p)
    }

    /// The active grade flash (rating + progress 0.0..1.0), for the card border color.
    pub fn grade_flash(&self) -> Option<(rs_fsrs::Rating, f64)> {
        let (rating, t) = self.grade_flash?;
        let p = t.elapsed().as_millis() as f64 / GRADE_FLASH_MS as f64;
        (p <= 1.0).then(|| (rating, p))
    }

    /// Whether a grade-undo is available right now (snapshot exists and within the
    /// 3s window). Drives the cmdmenu row's enabled state — was previously gated on
    /// `grade_flash()` (250ms), which made the 3s undo window unreachable.
    pub fn can_undo(&self) -> bool {
        self.undo_snap
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() <= Duration::from_secs(3))
    }

    /// Current Socratic-answer scroll offset (arrow keys in the Ask::Answer popup).
    pub fn ask_scroll(&self) -> usize {
        self.ask_scroll
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
        let info = self
            .scheduler
            .grade(anchor.card.clone(), rating, Utc::now());
        self.deck.save_card(&anchor.word, &info.card, true)?;
        self.deck
            .log_review(&anchor.word, rating, &info.review_log)?;
        if let Some(c) = &mut self.current {
            c.strike = Strike::Idle; // resolved — grading the new word is unblocked
        }
        if matches!(rating, rs_fsrs::Rating::Good) {
            if !self.reduced_motion {
                self.strike_anim = Some(Instant::now()); // fire the arc
            }
            self.toast = Some(ToastMsg::info(format!(
                "✦ 接线成功  {}  +1 复习",
                anchor.word
            )));
        } else {
            // Again/Hard on the anchor — the recall failed, so the OLD word lapses.
            // Symmetric to the Good message (✦ 成功 / ✗ 失败) and honest about the
            // lapse: this was a real FSRS review that counts as +1 lapse on anchor.
            self.toast = Some(ToastMsg::info(format!(
                "✗ 接线失败  {}  +1 lapse",
                anchor.word
            )));
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
            self.ask = Ask::Failed("未配置 DeepSeek 密钥（~/.tuna/config.toml）".to_string());
            return;
        }
        let word = c.entry.word.clone();
        let context = socratic_context(c);
        let (base, key, model) = (
            self.ds_base.clone(),
            self.ds_key.clone(),
            self.ds_chat_model.clone(),
        );
        self.ask_gen = self.ask_gen.wrapping_add(1);
        let gen_id = self.ask_gen;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let client = DeepSeek::new(base, key);
            let res = crate::llm::socratic::socratic(&client, &model, &word, &context)
                .map_err(|e| e.to_string());
            let _ = tx.send((gen_id, res));
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
        // Cross-platform open: macOS `open`, Linux `xdg-open`, Windows `cmd /c start`.
        let spawn = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        };
        match spawn {
            Ok(_) => self.toast = Some(ToastMsg::info(format!("↗ Wiktionary · {word}"))),
            Err(_) => self.toast = Some(ToastMsg::error(format!("无法打开浏览器 · {url}"))),
        }
    }

    /// Open the constellation — the current word's root-family, the words already lit
    /// in your galaxy and the frontier that's one root away. Nothing here is invented:
    /// every edge is a shared morpheme node, every glow is real FSRS stability.
    fn open_graph(&mut self) {
        let Some(word) = self.current.as_ref().map(|c| c.entry.word.clone()) else {
            return;
        };
        self.graph = self.deck.constellation(&word).unwrap_or_default();
        // A group with zero members leaves the overlay open with nothing selectable —
        // arrow keys no-op, Space/Enter hit None silently, and the only exit is g/Esc.
        // Guard against it: require at least one group with at least one member.
        let has_members = self.graph.iter().any(|g| !g.members.is_empty());
        if self.graph.is_empty() || !has_members {
            self.toast = Some(ToastMsg::info("这个词没有共享词根的邻居"));
            return;
        }
        self.graph_cursor = 0;
        self.show_graph = true;
    }

    /// Send the learner's OWN derivation guess to DeepSeek for a Socratic critique of
    /// his reasoning — the guess becomes a live channel, not a dead echo.
    /// Open the derivation chat overlay for the current new word. Doesn't send a
    /// message yet — the user types their guess and presses Enter to send. If no
    /// DeepSeek key is configured, show the failure immediately.
    fn start_derive_chat(&mut self) {
        if self.ds_key.is_empty() {
            self.toast = Some(ToastMsg::error(
                "未配置 DeepSeek 密钥（~/.tuna/config.toml）",
            ));
            return;
        }
        self.input.clear();
        self.derive_scroll = 0;
        self.derive = DeriveState::Open;
    }

    /// Send the current input as a new message in the derivation chat. Spawns a
    /// worker thread that calls the LLM with the full conversation history.
    fn send_derive_msg(&mut self) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() {
            return;
        }
        let Some(c) = self.current.as_ref() else {
            return;
        };
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
        // Stash the user message + history for the worker.
        let history: Vec<(bool, String)> = self
            .derive_turns
            .iter()
            .map(|t| (t.is_user, t.text.clone()))
            .collect();
        let (base, key, model) = (
            self.ds_base.clone(),
            self.ds_key.clone(),
            self.ds_chat_model.clone(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let send_msg = msg.clone();
        std::thread::spawn(move || {
            let client = DeepSeek::new(base, key);
            let res = crate::llm::socratic::derive_chat(
                &client, &model, &word, &morphemes, &history, &send_msg,
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(res);
        });
        // Record the user's message immediately so it shows in the chat view.
        self.derive_turns.push(ChatTurn {
            is_user: true,
            text: msg,
        });
        self.input.clear();
        self.derive_rx = Some(rx);
        self.derive = DeriveState::Pending;
    }

    /// Handle keys while the derivation chat overlay is open (or pending).
    fn on_derive_key(&mut self, key: KeyEvent) -> Result<()> {
        match (&self.derive, key.code) {
            (DeriveState::Closed, _) => {}
            (DeriveState::Pending, KeyCode::Esc) => {
                self.derive = DeriveState::Closed;
                self.derive_turns.clear();
                self.derive_rx = None;
            }
            (DeriveState::Pending, _) => {
                // Waiting for the LLM — only Esc works.
            }
            (DeriveState::Open, KeyCode::Esc) => {
                self.derive = DeriveState::Closed;
                self.derive_turns.clear();
            }
            (DeriveState::Open, KeyCode::Enter) => {
                if !self.input.trim().is_empty() {
                    self.send_derive_msg();
                }
            }
            (DeriveState::Open, KeyCode::Backspace) => {
                self.input.pop();
            }
            (DeriveState::Open, KeyCode::Up) => {
                self.derive_scroll = self.derive_scroll.saturating_sub(1);
            }
            (DeriveState::Open, KeyCode::Down) => {
                self.derive_scroll = self.derive_scroll.saturating_add(1);
            }
            (DeriveState::Open, KeyCode::Char(c)) if !c.is_control() => {
                self.input.push(c);
            }
            (DeriveState::Open, _) => {}
        }
        Ok(())
    }

    /// Drain any completed background work (Socratic answer, on-demand synth).
    pub fn poll_async(&mut self) {
        if let Some(rx) = &self.ask_rx {
            if let Ok((gen_id, res)) = rx.try_recv() {
                // Discard stale results: if the user cancelled (Esc/a) and reopened ask,
                // ask_gen has moved on and this result is from the old (still-billing)
                // worker. Dropping it stops a stale answer popping up over the new one.
                if gen_id == self.ask_gen && matches!(self.ask, Ask::Pending) {
                    self.ask = match res {
                        Ok(t) => Ask::Answer(t),
                        Err(e) => Ask::Failed(e),
                    };
                }
                self.ask_rx = None;
            }
        }
        // Derivation chat response — append the LLM reply to the conversation and
        // reopen the input. If the user closed the chat while pending, discard.
        if let Some(rx) = &self.derive_rx {
            if let Ok(res) = rx.try_recv() {
                if self.derive == DeriveState::Pending {
                    match res {
                        Ok(text) => {
                            self.derive_turns.push(ChatTurn {
                                is_user: false,
                                text,
                            });
                            self.derive = DeriveState::Open;
                        }
                        Err(e) => {
                            let first = e.lines().next().unwrap_or("请求失败");
                            self.toast = Some(ToastMsg::error(format!("推导对话失败: {first}")));
                            self.derive = DeriveState::Open;
                        }
                    }
                }
                self.derive_rx = None;
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
                        } else {
                            // The earphone dropped mid-synth — the audio is ready but
                            // there's nowhere to play it. Surface this so the spinner
                            // stopping isn't the only signal the user gets.
                            self.toast = Some(ToastMsg::warn("耳机断开 · 已丢弃合成"));
                        }
                    }
                    Err(e) => {
                        let first = e.lines().next().unwrap_or("synth failed").to_string();
                        self.toast = Some(ToastMsg::error(format!("合成失败: {first}")));
                    }
                }
            }
        }
        // Expire the Esc-to-quit priming window — if the user didn't follow up, the
        // next Esc starts fresh instead of quitting on a stale confirmation.
        if let Some(t) = self.esc_confirm {
            if t.elapsed() >= ESC_CONFIRM_MS {
                self.esc_confirm = None;
            }
        }
        // Auto-dismiss non-sticky toasts once their TTL elapses. `Error` toasts (TTL=None)
        // stay until the next keypress clears them in on_key.
        if let Some(t) = &self.toast {
            if t.expired() {
                self.toast = None;
            }
        }
        // Retire stale animation clocks so is_animating() can short-circuit and the
        // progress getters return None. The render path already clamps by elapsed, but
        // leaving Some(stale) around means every idle frame pays the branch cost.
        if let Some(t) = self.strike_anim {
            if t.elapsed().as_millis() >= STRIKE_ANIM_MS {
                self.strike_anim = None;
            }
        }
        if let Some((_, t)) = self.grade_flash {
            if t.elapsed().as_millis() >= GRADE_FLASH_MS {
                self.grade_flash = None;
            }
        }
        if let Some(t) = self.card_slide {
            if t.elapsed().as_millis() >= CARD_SLIDE_MS {
                self.card_slide = None;
            }
        }
        if let Some(t) = self.reveal_anim {
            let n = self.morpheme_count() as u128;
            let window = n * MORPHEME_STAGGER_MS + MORPHEME_CELL_FADE_MS;
            if t.elapsed().as_millis() >= window {
                self.reveal_anim = None;
            }
        }
    }

    fn grade(&mut self, n: u8) -> Result<()> {
        let Some(rating) = rating_from_u8(n) else {
            return Ok(());
        };
        if let Some(c) = self.current.take() {
            let was_new = c.is_new;
            // Snapshot the pre-grade card state for one-step undo. We keep the
            // DeckCard (FSRS state + word) and a timestamp; on undo we restore
            // pos to this card, rewrite its FSRS state, and reload. The 3s window
            // is short enough that the learner hasn't started engaging the next
            // card, but long enough to catch a reflexive wrong-key press.
            self.undo_snap = Some((c.dc.clone(), Instant::now()));
            let info = self.scheduler.grade(c.dc.card.clone(), rating, Utc::now());
            self.deck.save_card(&c.dc.word, &info.card, true)?;
            self.deck.log_review(&c.dc.word, rating, &info.review_log)?;
            if was_new {
                self.session_new += 1;
            } else {
                self.session_reviews += 1;
            }
        }
        if !self.reduced_motion {
            self.grade_flash = Some((rating, Instant::now()));
        }
        self.pos += 1;
        self.load_current()
    }

    /// Undo the most recent grade — restore the card, rewrite its FSRS state to
    /// the pre-grade snapshot, and reload it as current. One-step only: the snapshot
    /// is cleared on undo, so the learner can't walk back a chain of grades (that
    /// would let the displayed flow diverge from FSRS's review history). The 3s
    /// window covers the "pressed the wrong key, realized instantly" case.
    fn undo_grade(&mut self) -> Result<()> {
        const UNDO_WINDOW: Duration = Duration::from_secs(3);
        let Some((dc, t)) = self.undo_snap.take() else {
            self.toast = Some(ToastMsg::info("无可撤销的评分"));
            return Ok(());
        };
        if t.elapsed() > UNDO_WINDOW {
            self.toast = Some(ToastMsg::warn("已超时，不可撤回"));
            return Ok(());
        }
        // Rewrite the card's FSRS state to the pre-grade snapshot. The review log
        // row stays (it's append-only history) but the card state is restored, so
        // the next review uses the pre-grade stability/difficulty.
        self.deck.save_card(&dc.word, &dc.card, true)?;
        // Decrement the session counter that grade() incremented, so the done
        // screen and status bar stay accurate after an undo.
        if !dc.introduced {
            self.session_new = self.session_new.saturating_sub(1);
        } else {
            self.session_reviews = self.session_reviews.saturating_sub(1);
        }
        // Move pos back to the undone card and reload it as current.
        self.pos = self.pos.saturating_sub(1);
        self.load_current()?;
        // Clear the grade flash so it doesn't tint the restored card.
        self.grade_flash = None;
        self.toast = Some(ToastMsg::info("↶ 已撤销"));
        Ok(())
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
        let was_open = self.gate.open;
        let open_device = match probe::current_probe().enumerate() {
            Ok(devices) => probe::find_bound_output(&devices, &self.needle).map(|d| d.name.clone()),
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
        // Surface connect/disconnect transitions — in a silent office the status bar
        // is easy to miss, and the user deserves to know the audio path changed.
        if was_open && !self.gate.open {
            self.toast = Some(ToastMsg::warn("耳机断开 · 已静音"));
        } else if !was_open && self.gate.open {
            self.toast = Some(ToastMsg::info("耳机已连接"));
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

/// Render a `chrono::Duration` as a compact Chinese human interval.
/// Overdue cards (mins ≤ 0) read "现在" so they're distinguishable from a 1-minute
/// ahead card (was both "1m" — the exam-prep user couldn't tell "due now" from "due
/// in a minute"). Units are 中文一致 (分/时/天) to match the rest of the UI.
fn human_interval(d: chrono::Duration) -> String {
    let mins = d.num_minutes();
    if mins <= 0 {
        return "现在".to_string();
    }
    if mins < 60 {
        format!("{}分", mins.max(1))
    } else if mins < 60 * 24 {
        format!("{}时", mins / 60)
    } else {
        format!("{}天", mins / (60 * 24))
    }
}
