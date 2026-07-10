//! Kokoro multi-lang (zh+en) engine descriptor — the chat voice.
//!
//! AI replies are Chinese prose with embedded English words and morpheme fragments
//! ("spect"、"-ate"). This is the one sherpa zh+en model whose frontend falls back
//! to espeak-ng G2P for out-of-lexicon English (lexicon-only frontends like MeloTTS
//! silently DROP such tokens — which here are exactly the words being taught).
//! Not a study engine: `TtsEngineKind::all()` excludes it, so the settings overlay
//! and the wizard's study-engine picker never offer it as the word pronouncer.

use std::path::{Path, PathBuf};

use super::{Download, EngineFiles, Voice};

pub struct KokoroZhEngine;

// fp32, not the int8 export: int8 emits NaN samples on macOS arm64 (verified —
// silent audio, rms NaN), fp32 synthesizes correctly. 348MB vs 140MB is the price
// of audio that actually plays.
pub const SUBDIR: &str = "kokoro-multi-lang-v1_1";
const MODEL: &str = "model.onnx";
const VOICES: &str = "voices.bin";
const TOKENS: &str = "tokens.txt";
const LEXICON_EN: &str = "lexicon-us-en.txt";
const LEXICON_ZH: &str = "lexicon-zh.txt";
const ESPEAK: &str = "espeak-ng-data";
const FST_DATE: &str = "date-zh.fst";
const FST_NUMBER: &str = "number-zh.fst";
const DICT: &str = "dict";

const TARBALL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2";
const TARBALL_MB: usize = 348;

impl KokoroZhEngine {
    pub fn files(dir: &Path) -> EngineFiles {
        let root = dir.join(SUBDIR);
        EngineFiles {
            model: root.join(MODEL),
            tokens: root.join(TOKENS),
            data_dir: root.join(ESPEAK),
            voices: Some(root.join(VOICES)),
            vocoder: None,
            // Both lexicons, comma-joined into one config string by the session —
            // English first so lexicon hits cover common words, espeak covers OOV.
            lexicons: vec![root.join(LEXICON_EN), root.join(LEXICON_ZH)],
            // Normalize dates/numbers into Chinese words before G2P; without these
            // FSTs "2015" reads as OOV token soup.
            rule_fsts: vec![root.join(FST_DATE), root.join(FST_NUMBER)],
            // Jieba segmentation for the Chinese half — REQUIRED: without it the
            // frontend degrades to the generic lexicon and the model emits NaNs.
            dict_dir: Some(root.join(DICT)),
        }
    }
}

impl super::TtsEngine for KokoroZhEngine {
    fn voices(&self) -> Vec<Voice> {
        // v1_1 sid map: 0-1 af_maple/af_sol, 2 bf_vale, 3-57 zf_* (Chinese female),
        // 58-102 zm_* (Chinese male). Replies are Chinese-dominant prose, so the
        // default is a Chinese female voice.
        vec![
            Voice {
                id: "zh_female".into(),
                sid: 3,
            },
            Voice {
                id: "zh_male".into(),
                sid: 58,
            },
            Voice {
                id: "af_maple".into(),
                sid: 0,
            },
        ]
    }

    fn default_voice(&self) -> Voice {
        self.voices().into_iter().next().unwrap()
    }

    fn downloads(&self) -> Vec<Download> {
        vec![Download {
            url: TARBALL_URL.into(),
            label: "kokoro-multi-lang-v1_1.tar.bz2".into(),
            dest: PathBuf::from("kokoro-multi-lang-v1_1.tar.bz2"),
        }]
    }

    fn footprint_mb(&self) -> usize {
        TARBALL_MB
    }

    fn models_present(&self, dir: &Path) -> bool {
        let f = Self::files(dir);
        f.model.exists()
            && f.tokens.exists()
            && f.data_dir.exists()
            && f.voices.as_ref().is_some_and(|p| p.exists())
            && f.lexicons.iter().all(|p| p.exists())
            && f.rule_fsts.iter().all(|p| p.exists())
            && f.dict_dir.as_ref().is_some_and(|p| p.exists())
    }

    fn blurb(&self) -> &'static str {
        "Kokoro 多语 · 中英混合语音（AI 对话朗读）· ~350MB"
    }
}
