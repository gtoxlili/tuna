//! Piper engine descriptor — sherpa's `OfflineTtsVitsModelConfig` path.
//!
//! Piper is a VITS model family trained by the community on diverse voice datasets
//! (Ryan, Lessac, Alba, …) under MIT licence. Each Piper tarball is one voice;
//! switching voices means swapping the model file. espeak-ng-data drives G2P.

use std::path::{Path, PathBuf};

use super::{Download, EngineFiles, Voice};

pub struct PiperEngine;

/// We ship one Piper voice tarball (Lessac medium) as the default; the subdir name
/// matches the tarball's extracted directory.
pub const SUBDIR: &str = "vits-piper-en_US-lessac-medium";
const MODEL: &str = "en_US-lessac-medium.onnx";
const TOKENS: &str = "tokens.txt";
const ESPEAK: &str = "espeak-ng-data";

const TARBALL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-lessac-medium.tar.bz2";
const TARBALL_MB: usize = 63;

impl PiperEngine {
    pub fn files(dir: &Path) -> EngineFiles {
        let root = dir.join(SUBDIR);
        EngineFiles {
            model: root.join(MODEL),
            tokens: root.join(TOKENS),
            data_dir: root.join(ESPEAK),
            voices: None,
            vocoder: None,
            lexicon: None,
        }
    }
}

impl super::TtsEngine for PiperEngine {
    fn voices(&self) -> Vec<Voice> {
        // Piper models are single-speaker; sid 0 is the only voice. The bundled
        // Lessac-medium model is a calm American female voice.
        vec![Voice {
            id: "lessac".into(),
            sid: 0,
        }]
    }

    fn default_voice(&self) -> Voice {
        self.voices().into_iter().next().unwrap()
    }

    fn downloads(&self) -> Vec<Download> {
        vec![Download {
            url: TARBALL_URL.into(),
            label: "vits-piper-en_US-lessac-medium.tar.bz2".into(),
            dest: PathBuf::from("vits-piper-en_US-lessac-medium.tar.bz2"),
        }]
    }

    fn footprint_mb(&self) -> usize {
        TARBALL_MB
    }

    fn models_present(&self, dir: &Path) -> bool {
        let f = Self::files(dir);
        f.model.exists() && f.tokens.exists() && f.data_dir.exists()
    }

    fn blurb(&self) -> &'static str {
        "Piper VITS · 社区多音色 · Lessac 女声 · ~63MB"
    }
}
