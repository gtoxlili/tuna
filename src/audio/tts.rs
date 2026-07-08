//! Text-to-speech via a Kokoro uv sidecar, pre-synthesized to a WAV cache.
//!
//! The deck is finite, so we synthesize offline and the TUI only ever *plays*
//! cached files through the earphone gate — runtime latency ~0, and nothing makes
//! a sound unless the bound earphone is present and you press play.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{ensure, Context, Result};

pub struct Tts {
    pub cache_dir: PathBuf,
    pub model: PathBuf,
    pub voices: PathBuf,
    pub voice: String,
    pub speed: f32,
    pub sidecar: PathBuf,
}

impl Tts {
    /// Stable content-addressed path for a clip (deterministic across runs).
    pub fn cache_path(&self, text: &str) -> PathBuf {
        let mut h = DefaultHasher::new();
        text.trim().hash(&mut h);
        self.voice.hash(&mut h);
        ((self.speed * 100.0) as i32).hash(&mut h);
        self.cache_dir.join(format!("{:016x}.wav", h.finish()))
    }

    pub fn is_cached(&self, text: &str) -> bool {
        self.cache_path(text).exists()
    }

    pub fn models_present(&self) -> bool {
        self.model.exists() && self.voices.exists()
    }

    /// Synthesize any of `texts` not already cached, via one sidecar run (one model load).
    /// Returns how many new clips were requested.
    pub fn synth_batch(&self, texts: &[String]) -> Result<usize> {
        ensure!(
            self.models_present(),
            "Kokoro model not found at {} / {} — download it (see README).",
            self.model.display(),
            self.voices.display()
        );
        let mut jobs = Vec::new();
        for t in texts {
            let t = t.trim();
            if t.is_empty() || self.is_cached(t) {
                continue;
            }
            jobs.push(serde_json::json!({
                "text": t,
                "out": self.cache_path(t),
                "voice": self.voice,
                "speed": self.speed,
            }));
        }
        if jobs.is_empty() {
            return Ok(0);
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        let jobfile = self.cache_dir.join("_jobs.json");
        std::fs::write(&jobfile, serde_json::to_string(&jobs)?)?;

        let status = Command::new("uv")
            .arg("run")
            .arg(&self.sidecar)
            .arg(&jobfile)
            .env("KOKORO_MODEL", &self.model)
            .env("KOKORO_VOICES", &self.voices)
            .status()
            .context("running the uv synth sidecar (is `uv` installed?)")?;
        ensure!(status.success(), "synth sidecar exited with an error");
        Ok(jobs.len())
    }
}
