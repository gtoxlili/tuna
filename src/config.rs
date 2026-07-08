//! Config lives at ~/.tuna/config.toml and holds only preferences (key, earphone,
//! voice). All file *locations* come from `paths`, so the config is portable as-is.
//! `DEEPSEEK_API_KEY` in the environment overrides the file.

use anyhow::{bail, Result};
use serde::Deserialize;

use crate::audio::tts::Tts;
use crate::paths;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub deepseek: DeepSeekCfg,
    pub gate: GateCfg,
    pub tts: TtsCfg,
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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct TtsCfg {
    pub voice: String,
    pub speed: f32,
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
impl Default for TtsCfg {
    fn default() -> Self {
        Self {
            voice: "af_heart".to_string(),
            speed: 1.0,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut cfg = if paths::config_file().exists() {
            toml::from_str(&std::fs::read_to_string(paths::config_file())?)?
        } else {
            Config::default()
        };
        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            if !key.is_empty() {
                cfg.deepseek.api_key = key;
            }
        }
        Ok(cfg)
    }

    /// A TTS engine wired to the ~/.tuna locations.
    pub fn tts_engine(&self) -> Tts {
        Tts {
            cache_dir: paths::audio_cache(),
            model: paths::kokoro_model(),
            voices: paths::kokoro_voices(),
            voice: self.tts.voice.clone(),
            speed: self.tts.speed,
            sidecar: paths::root().join("synth.py"),
        }
    }

    pub fn require_key(&self) -> Result<&str> {
        if self.deepseek.api_key.is_empty() {
            bail!(
                "no DeepSeek key — set it in {} ([deepseek] api_key = \"…\") or $DEEPSEEK_API_KEY",
                paths::config_file().display()
            );
        }
        Ok(&self.deepseek.api_key)
    }
}

/// The config.toml written on first run.
pub const TEMPLATE: &str = r#"# tuna 配置 · ~/.tuna/config.toml
# DeepSeek 密钥用于词条精加工与苏格拉底辨析;学习本身离线可用,无需密钥。
# 也可用环境变量 DEEPSEEK_API_KEY 覆盖。

[deepseek]
api_key = ""
base_url = "https://api.deepseek.com"
enrich_model = "deepseek-v4-flash"
chat_model = "deepseek-v4-pro"

[gate]
# 绑定耳机的名字子串(只在连着它时才发声)
needle = "airpods"

[tts]
voice = "af_heart"
speed = 1.0
"#;
