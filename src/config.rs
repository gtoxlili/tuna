//! Local config: `tuna.toml` (gitignored) holds the DeepSeek key + model choices
//! and the bound earphone. `DEEPSEEK_API_KEY` in the environment overrides the file.

use std::path::Path;

use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub deepseek: DeepSeekCfg,
    pub gate: GateCfg,
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

    /// The DeepSeek key, or a clear error pointing at how to set it.
    pub fn require_key(&self) -> Result<&str> {
        if self.deepseek.api_key.is_empty() {
            bail!("no DeepSeek API key — set it in tuna.toml ([deepseek] api_key = \"…\") or $DEEPSEEK_API_KEY");
        }
        Ok(&self.deepseek.api_key)
    }
}
