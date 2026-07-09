//! TTS via sherpa-onnx — a single statically-linked C++ lib covering Kokoro / Matcha /
//! Piper under one `OfflineTts` API. Replaces the ort + misaki-rs Kokoro-only path.
//!
//! Pipeline: text → sherpa `generate_with_config` (internal espeak-ng-data G2P) →
//! f32 samples → `sherpa_onnx::write` → cached WAV → earphone gate playback.
//!
//! `TtsEngine` is the static descriptor (URLs / voices / footprint) for setup and
//! the settings overlay; `TtsConfig` is the clonable value carried into worker threads;
//! `SynthSession` is the warm `OfflineTts` instance that does the actual synth.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Bumped when the synth pipeline changes shape, so cached clips from an older
/// pipeline are never served by mistake (e.g. the ort→sherpa migration invalidates
/// every `{word}.wav` from the old Kokoro path).
pub const PIPELINE_VERSION: &str = "sherpa-v1";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngineKind {
    Kokoro,
    Matcha,
    Piper,
}

impl TtsEngineKind {
    pub fn id(self) -> &'static str {
        match self {
            TtsEngineKind::Kokoro => "kokoro",
            TtsEngineKind::Matcha => "matcha",
            TtsEngineKind::Piper => "piper",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "kokoro" => Some(TtsEngineKind::Kokoro),
            "matcha" => Some(TtsEngineKind::Matcha),
            "piper" => Some(TtsEngineKind::Piper),
            _ => None,
        }
    }

    pub fn all() -> [TtsEngineKind; 3] {
        [TtsEngineKind::Kokoro, TtsEngineKind::Matcha, TtsEngineKind::Piper]
    }
}

/// One downloadable artefact for an engine (model tarball, vocoder, espeak-ng-data).
#[derive(Clone)]
pub struct Download {
    pub url: String,
    pub label: String,
    /// Where this artefact goes *relative to the engine dir*. `.tar.bz2` archives are
    /// extracted into the engine dir (their internal `SUBDIR/` lands correctly); any
    /// other file is placed directly at this relative path.
    pub dest: PathBuf,
}

/// The resolved file layout of an engine under its directory. Built by each engine's
/// `files(dir)` helper, consumed by both `models_present` and the synth session's
/// sherpa config construction — so the two never disagree about where files live.
pub struct EngineFiles {
    pub model: PathBuf,
    pub tokens: PathBuf,
    pub data_dir: PathBuf,
    /// Kokoro's `voices.bin`.
    pub voices: Option<PathBuf>,
    /// Matcha's separate vocoder ONNX.
    pub vocoder: Option<PathBuf>,
    /// Matcha/Piper lexicon (optional depending on the tarball).
    pub lexicon: Option<PathBuf>,
}

/// A speaker the engine can synthesize as. `sid` is sherpa's integer speaker id;
/// `id` is the human-stable handle persisted in config (e.g. "af_heart").
#[derive(Clone)]
pub struct Voice {
    pub id: String,
    pub sid: i32,
}

/// Static descriptor for one engine: where to fetch it, which voices it ships, how
/// big it is. Lives behind a `Box<dyn TtsEngine>` and never holds runtime state.
pub trait TtsEngine: Send {
    fn voices(&self) -> Vec<Voice>;
    fn default_voice(&self) -> Voice;
    fn downloads(&self) -> Vec<Download>;
    fn footprint_mb(&self) -> usize;
    /// Whether every required file for this engine is present under `dir`.
    fn models_present(&self, dir: &Path) -> bool;
    /// A one-line paradigm blurb for the setup wizard's engine picker.
    fn blurb(&self) -> &'static str;
}

/// The clonable value carried into worker threads — no `Box<dyn TtsEngine>` crosses
/// the thread boundary, so the closure stays `'static` and cheap.
#[derive(Clone)]
pub struct TtsConfig {
    pub kind: TtsEngineKind,
    pub voice: String,
    pub speed: f32,
    pub cache_dir: PathBuf,
    pub engine_dir: PathBuf,
}

impl TtsConfig {
    /// Content-addressed clip path. Keyed on text + voice + speed + engine +
    /// pipeline version, so switching engine or bumping the pipeline never serves a
    /// stale clip from another voice/model.
    pub fn cache_path(&self, text: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        text.trim().hash(&mut h);
        self.voice.hash(&mut h);
        ((self.speed * 100.0) as i32).hash(&mut h);
        self.kind.id().hash(&mut h);
        PIPELINE_VERSION.hash(&mut h);
        self.cache_dir.join(format!("{:016x}.wav", h.finish()))
    }

    pub fn models_present(&self) -> bool {
        from_kind(self.kind).models_present(&self.engine_dir)
    }
}

/// Build the static engine descriptor for `kind`.
pub fn from_kind(kind: TtsEngineKind) -> Box<dyn TtsEngine> {
    match kind {
        TtsEngineKind::Kokoro => Box::new(kokoro::KokoroEngine),
        TtsEngineKind::Matcha => Box::new(matcha::MatchaEngine),
        TtsEngineKind::Piper => Box::new(piper::PiperEngine),
    }
}

/// Start a warm synth session for `cfg`. The returned `Box<dyn SynthSession>` owns
/// the sherpa `OfflineTts` and is `Send`, so it can live in an `Arc<Mutex<Option<…>>>`
/// on the UI thread and be locked from a worker thread for on-demand synth.
pub fn start_session(cfg: &TtsConfig) -> Result<Box<dyn SynthSession>> {
    session::SherpaSession::start(cfg)
}

/// A warm synth engine: holds the sherpa `OfflineTts` and the voice list, and runs
/// `generate_with_config` on demand. `OfflineTts` is `Send + Sync`, so this is safe
/// to share across threads via `Arc<Mutex<Option<Box<dyn SynthSession>>>>`.
pub trait SynthSession: Send {
    /// Synthesize `text` to WAV at `out` (blocking). First call pays graph optimize.
    fn synth(&mut self, text: &str, out: &Path, voice: &str, speed: f32) -> Result<()>;
    /// Synthesize and return the raw f32 samples + sample rate (dev stats / custom sink).
    fn synth_raw(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, i32)>;
}

pub mod kokoro;
pub mod matcha;
pub mod piper;
pub mod session;
