//! Local config: `tuna.toml` (gitignored) holds the DeepSeek key + model choices
//! and the bound earphone. `DEEPSEEK_API_KEY` in the environment overrides the file.

use std::path::Path;

use anyhow::{bail, Result};
use serde::Deserialize;

use crate::audio::tts::Tts;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub deepseek: DeepSeekCfg,
    pub gate: GateCfg,
    pub tts: TtsCfg,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TtsCfg {
    pub model: String,
    pub voices: String,
    pub voice: String,
    pub speed: f32,
    pub cache_dir: String,
    pub sidecar: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DeepSeekCfg {
    pub api_key: String,
    pub base_url: String,
    pub enrich_model: String,
    pub chat_model: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct GateCfg {
    pub needle: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            deepseek: DeepSeekCfg::default(),
            gate: GateCfg::default(),
            tts: TtsCfg::default(),
        }
    }
}

impl Default for TtsCfg {
    fn default() -> Self {
        Self {
            model: "data/tts/models/kokoro-v1.0.int8.onnx".to_string(),
            voices: "data/tts/models/voices-v1.0.bin".to_string(),
            voice: "af_heart".to_string(),
            speed: 1.0,
            cache_dir: "cache/audio".to_string(),
            sidecar: "sidecar/synth.py".to_string(),
        }
    }
}

impl Default for DeepSeekCfg {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.deepseek.com".to_string(),
            enrich_model: "deepseek-v4-flash".to_string(),
            chat_model: "deepseek-v4-pro".to_string(),
        }
    }
}

impl Default for GateCfg {
    fn default() -> Self {
        Self {
            needle: "airpods".to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut cfg = if Path::new("tuna.toml").exists() {
            let s = std::fs::read_to_string("tuna.toml")?;
            toml::from_str(&s)?
        } else {
            Config::default()
        };
        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            cfg.deepseek.api_key = key;
        }
        Ok(cfg)
    }

    /// Build a TTS engine from config.
    pub fn tts_engine(&self) -> Tts {
        Tts {
            cache_dir: self.tts.cache_dir.clone().into(),
            model: self.tts.model.clone().into(),
            voices: self.tts.voices.clone().into(),
            voice: self.tts.voice.clone(),
            speed: self.tts.speed,
            sidecar: self.tts.sidecar.clone().into(),
        }
    }

    /// The DeepSeek key, or a clear error pointing at how to set it.
    pub fn require_key(&self) -> Result<&str> {
        if self.deepseek.api_key.is_empty() {
            bail!("no DeepSeek API key — set it in tuna.toml ([deepseek] api_key = \"…\") or $DEEPSEEK_API_KEY");
        }
        Ok(&self.deepseek.api_key)
    }
}
