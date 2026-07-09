//! Matcha-TTS engine descriptor — sherpa's `OfflineTtsMatchaModelConfig` path.
//!
//! Matcha uses optimal-conditional flow matching (OT-CFM), a synthesis paradigm
//! distinct from Kokoro's style-vector approach. sherpa separates the acoustic
//! model from the vocoder (HiFiGAN), so the tarball carries both. espeak-ng-data
//! drives G2P the same way as Kokoro.

use std::path::{Path, PathBuf};

use super::{Download, EngineFiles, Voice};

pub struct MatchaEngine;

pub const SUBDIR: &str = "matcha-icefall-en_US-ljspeech";
const ACOUSTIC: &str = "model-steps-3.onnx";
const VOCODER: &str = "hifigan_v2.onnx";
const TOKENS: &str = "tokens.txt";
const ESPEAK: &str = "espeak-ng-data";
const LEXICON: &str = "lexicon.txt";

const TARBALL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/matcha-icefall-en_US-ljspeech.tar.bz2";
const VOCODER_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/vocoder-models/hifigan_v2.onnx";
const TARBALL_MB: usize = 220;

impl MatchaEngine {
    pub fn files(dir: &Path) -> EngineFiles {
        let root = dir.join(SUBDIR);
        EngineFiles {
            model: root.join(ACOUSTIC),
            tokens: root.join(TOKENS),
            data_dir: root.join(ESPEAK),
            voices: None,
            vocoder: Some(root.join(VOCODER)),
            lexicon: Some(root.join(LEXICON)),
        }
    }
}

impl super::TtsEngine for MatchaEngine {
    fn voices(&self) -> Vec<Voice> {
        // Matcha (LJSpeech) is a single-speaker model; sid 0 is the only voice.
        vec![Voice {
            id: "ljspeech".into(),
            sid: 0,
        }]
    }

    fn default_voice(&self) -> Voice {
        self.voices().into_iter().next().unwrap()
    }

    fn downloads(&self) -> Vec<Download> {
        vec![
            Download {
                url: TARBALL_URL.into(),
                label: "matcha-icefall-en_US-ljspeech.tar.bz2".into(),
                dest: PathBuf::from("matcha-icefall-en_US-ljspeech.tar.bz2"),
            },
            Download {
                url: VOCODER_URL.into(),
                label: "hifigan_v2.onnx".into(),
                dest: PathBuf::from(SUBDIR).join(VOCODER),
            },
        ]
    }

    fn footprint_mb(&self) -> usize {
        TARBALL_MB + 64
    }

    fn models_present(&self, dir: &Path) -> bool {
        let f = Self::files(dir);
        f.model.exists()
            && f.tokens.exists()
            && f.data_dir.exists()
            && f.vocoder.as_ref().is_some_and(|v| v.exists())
    }

    fn blurb(&self) -> &'static str {
        "Matcha-TTS · 条件流匹配范式 · LJSpeech 女声 · ~280MB"
    }
}
