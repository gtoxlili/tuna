//! Kokoro engine descriptor — sherpa's `OfflineTtsKokoroModelConfig` path.
//!
//! sherpa ships its own ONNX export of Kokoro (distinct from thewh1teagle's):
//! inputs are `tokens` + `voices` + `style`, G2P is espeak-ng-data (not misaki),
//! and voice selection is by integer `sid` into `voices.bin`. The tarball bundles
//! `model.onnx`, `voices.bin`, `tokens.txt`, and `espeak-ng-data/`.

use std::path::{Path, PathBuf};

use super::{Download, EngineFiles, Voice};

pub struct KokoroEngine;

/// The subdirectory created when the tarball is extracted under the engine dir.
pub const SUBDIR: &str = "kokoro-en-v0_19";
const MODEL: &str = "model.onnx";
const VOICES: &str = "voices.bin";
const TOKENS: &str = "tokens.txt";
const ESPEAK: &str = "espeak-ng-data";

const TARBALL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-en-v0_19.tar.bz2";
const TARBALL_MB: usize = 320;

impl KokoroEngine {
    /// Resolve the engine's file layout under `dir` (the per-engine subdirectory of
    /// `~/.tuna/tts/`). Both `models_present` and the session's sherpa config use this.
    pub fn files(dir: &Path) -> EngineFiles {
        let root = dir.join(SUBDIR);
        EngineFiles {
            model: root.join(MODEL),
            tokens: root.join(TOKENS),
            data_dir: root.join(ESPEAK),
            voices: Some(root.join(VOICES)),
            vocoder: None,
            lexicons: Vec::new(),
            rule_fsts: Vec::new(),
            dict_dir: None,
        }
    }
}

impl super::TtsEngine for KokoroEngine {
    fn voices(&self) -> Vec<Voice> {
        // kokoro-en-v0_19 ships 11 speakers; sid order per the sherpa model docs.
        // (`af_heart` is a Kokoro v1.0 voice — it does not exist in this export.)
        const IDS: [&str; 11] = [
            "af",
            "af_bella",
            "af_nicole",
            "af_sarah",
            "af_sky",
            "am_adam",
            "am_michael",
            "bf_emma",
            "bf_isabella",
            "bm_george",
            "bm_lewis",
        ];
        IDS.iter()
            .enumerate()
            .map(|(i, id)| Voice {
                id: (*id).into(),
                sid: i as i32,
            })
            .collect()
    }

    fn default_voice(&self) -> Voice {
        self.voices().into_iter().next().unwrap()
    }

    fn downloads(&self) -> Vec<Download> {
        vec![Download {
            url: TARBALL_URL.into(),
            label: "kokoro-en-v0_19.tar.bz2".into(),
            dest: PathBuf::from("kokoro-en-v0_19.tar.bz2"),
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
            && f.voices.as_ref().is_some_and(|v| v.exists())
    }

    fn blurb(&self) -> &'static str {
        "Kokoro-82M · 风格向量 TTS · 英文女声 · ~320MB"
    }
}
