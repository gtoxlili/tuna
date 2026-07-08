//! Text-to-speech via a Kokoro uv sidecar, pre-synthesized to a WAV cache.
//!
//! The deck is finite, so we synthesize offline and the TUI only ever *plays*
//! cached files through the earphone gate — runtime latency ~0, and nothing makes
//! a sound unless the bound earphone is present and you press play.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{bail, ensure, Context, Result};

#[derive(Clone)]
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

/// A warm, long-running Kokoro process. The model loads once and stays resident,
/// so on-demand synthesis is fast after the first call — no offline pre-synth.
/// Driven from a worker thread (its calls block on child I/O).
pub struct TtsServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl TtsServer {
    pub fn start(tts: &Tts) -> Result<Self> {
        ensure!(
            tts.models_present(),
            "Kokoro model not found at {} — download it (see README).",
            tts.model.display()
        );
        let mut child = Command::new("uv")
            .arg("run")
            .arg(&tts.sidecar)
            .arg("--server")
            .env("KOKORO_MODEL", &tts.model)
            .env("KOKORO_VOICES", &tts.voices)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning the uv tts server (is `uv` installed?)")?;
        let stdin = child.stdin.take().context("tts server stdin")?;
        let mut stdout = BufReader::new(child.stdout.take().context("tts server stdout")?);
        // Consume the {"ready":true} line (blocks while uv resolves the env).
        let mut ready = String::new();
        stdout.read_line(&mut ready)?;
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    /// Synthesize `text` to `out` (blocking). First call pays the model load.
    pub fn synth(&mut self, text: &str, out: &Path, voice: &str, speed: f32) -> Result<()> {
        let req = serde_json::json!({ "text": text, "out": out, "voice": voice, "speed": speed });
        writeln!(self.stdin, "{req}").context("writing to tts server")?;
        self.stdin.flush().ok();
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).context("reading tts server")?;
        ensure!(n > 0, "tts server closed unexpectedly");
        let resp: serde_json::Value = serde_json::from_str(line.trim())
            .with_context(|| format!("tts server response: {line}"))?;
        if resp["ok"].as_bool() != Some(true) {
            bail!("synth failed: {}", resp["error"].as_str().unwrap_or("unknown"));
        }
        Ok(())
    }
}

impl Drop for TtsServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
