//! The warm synth session — owns a sherpa `OfflineTts` and runs `generate_with_config`
//! on demand. One implementation serves all three engines; only the sherpa config
//! construction differs by `TtsEngineKind`.
//!
//! `OfflineTts` is `Send + Sync`, so the session can sit in an
//! `Arc<Mutex<Option<Box<dyn SynthSession>>>>` and be locked from a worker thread.
//! `GeneratedAudio` is `!Send`, so samples are copied to a `Vec<f32>` inside `synth`
//! before the WAV is written — nothing cross-thread.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsMatchaModelConfig, OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
};

use super::{SynthSession, TtsConfig, TtsEngineKind, Voice, kokoro, kokoro_zh, matcha, piper};

pub struct SherpaSession {
    tts: OfflineTts,
    voices: Vec<Voice>,
}

impl SherpaSession {
    /// Build the sherpa config for `kind` from the engine's resolved file layout,
    /// create the `OfflineTts`, and return a boxed session ready for on-demand synth.
    pub fn start(cfg: &TtsConfig) -> Result<Box<dyn SynthSession>> {
        let files = match cfg.kind {
            TtsEngineKind::Kokoro => kokoro::KokoroEngine::files(&cfg.engine_dir),
            TtsEngineKind::Matcha => matcha::MatchaEngine::files(&cfg.engine_dir),
            TtsEngineKind::Piper => piper::PiperEngine::files(&cfg.engine_dir),
            TtsEngineKind::KokoroZh => kokoro_zh::KokoroZhEngine::files(&cfg.engine_dir),
        };
        let voices = super::from_kind(cfg.kind).voices();
        let model_config = build_model_config(cfg.kind, &files);
        // Text-normalization FSTs are a top-level OfflineTtsConfig concern (the
        // zh chat voice normalizes dates/numbers into Chinese words with them).
        let rule_fsts = join_paths(&files.rule_fsts);
        let config = OfflineTtsConfig {
            model: model_config,
            rule_fsts,
            ..Default::default()
        };
        let tts = OfflineTts::create(&config)
            .context("sherpa OfflineTts::create returned None — model files missing or invalid")?;
        Ok(Box::new(Self { tts, voices }))
    }
}

impl SynthSession for SherpaSession {
    fn synth(&mut self, text: &str, out: &Path, voice: &str, speed: f32) -> Result<()> {
        let (samples, sample_rate) = self.synth_raw(text, voice, speed)?;
        let path = out
            .to_str()
            .ok_or_else(|| anyhow!("WAV path is not UTF-8: {}", out.display()))?;
        if !sherpa_onnx::write(path, &samples, sample_rate) {
            return Err(anyhow!("sherpa write failed for {}", out.display()));
        }
        Ok(())
    }

    fn synth_raw(&mut self, text: &str, voice: &str, speed: f32) -> Result<(Vec<f32>, i32)> {
        let sid = self
            .voices
            .iter()
            .find(|v| v.id == voice)
            .map(|v| v.sid)
            .unwrap_or(0);
        let gen_cfg = GenerationConfig {
            sid,
            speed,
            ..Default::default()
        };
        let audio = self
            .tts
            .generate_with_config(text, &gen_cfg, None::<fn(&[f32], f32) -> bool>)
            .context("sherpa generate_with_config returned None")?;
        // GeneratedAudio is !Send; copy samples here, then drop it.
        Ok((audio.samples().to_vec(), audio.sample_rate()))
    }
}

/// Comma-join a path list into sherpa's one-string list convention, or None when
/// the engine carries none.
fn join_paths(paths: &[std::path::PathBuf]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    Some(
        paths
            .iter()
            .map(|p| p.to_str().expect("path not UTF-8").to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Translate the engine's file layout into sherpa's nested config, filling only the
/// arm for the chosen engine and leaving the rest at default.
fn build_model_config(kind: TtsEngineKind, f: &super::EngineFiles) -> OfflineTtsModelConfig {
    let path_str = |p: &Path| p.to_str().expect("path not UTF-8").to_string();
    match kind {
        // Both Kokoro exports share the config shape; the zh+en one adds the
        // comma-joined en+zh lexicons (espeak-ng-data covers OOV English G2P) and
        // the jieba dict_dir for Chinese segmentation.
        TtsEngineKind::Kokoro | TtsEngineKind::KokoroZh => OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(path_str(&f.model)),
                voices: f.voices.as_ref().map(|p| path_str(p)),
                tokens: Some(path_str(&f.tokens)),
                data_dir: Some(path_str(&f.data_dir)),
                lexicon: join_paths(&f.lexicons),
                dict_dir: f.dict_dir.as_ref().map(|p| path_str(p)),
                ..Default::default()
            },
            num_threads: 1,
            ..Default::default()
        },
        TtsEngineKind::Matcha => OfflineTtsModelConfig {
            matcha: OfflineTtsMatchaModelConfig {
                acoustic_model: Some(path_str(&f.model)),
                vocoder: f.vocoder.as_ref().map(|p| path_str(p)),
                tokens: Some(path_str(&f.tokens)),
                data_dir: Some(path_str(&f.data_dir)),
                lexicon: join_paths(&f.lexicons),
                ..Default::default()
            },
            num_threads: 1,
            ..Default::default()
        },
        TtsEngineKind::Piper => OfflineTtsModelConfig {
            vits: OfflineTtsVitsModelConfig {
                model: Some(path_str(&f.model)),
                tokens: Some(path_str(&f.tokens)),
                data_dir: Some(path_str(&f.data_dir)),
                ..Default::default()
            },
            num_threads: 1,
            ..Default::default()
        },
    }
}
