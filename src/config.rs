//! Config lives at ~/.tuna/config.toml and holds only preferences (key, earphone,
//! voice). All file *locations* come from `paths`, so the config is portable as-is.
//! `DEEPSEEK_API_KEY` in the environment overrides the file.

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::audio::tts::{TtsConfig, TtsEngineKind};
use crate::paths;

#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub deepseek: DeepSeekCfg,
    pub gate: GateCfg,
    pub tts: TtsCfg,
    pub a11y: A11yCfg,
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
    pub engine: String,
    pub voice: String,
    pub speed: f32,
}

/// Accessibility preferences. `reduced_motion` skips all animation (strike arc,
/// grade flash, card slide, morpheme stagger) — for vestibular sensitivity or terminals
/// where animation flicker is unwelcome.
#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct A11yCfg {
    pub reduced_motion: bool,
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
            engine: "kokoro".to_string(),
            // sid 0 of kokoro-en-v0_19 — "af_heart" belongs to Kokoro v1.0 and does
            // not exist in this export; an unknown name silently falls back to sid 0
            // anyway, so the honest default is the real sid-0 name.
            voice: "af".to_string(),
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
        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY")
            && !key.is_empty() {
                cfg.deepseek.api_key = key;
            }
        Ok(cfg)
    }

    /// The TTS value config wired to the ~/.tuna locations, clonable into worker threads.
    pub fn tts_engine(&self) -> TtsConfig {
        let kind = TtsEngineKind::from_id(&self.tts.engine).unwrap_or(TtsEngineKind::Kokoro);
        TtsConfig {
            kind,
            voice: self.tts.voice.clone(),
            speed: self.tts.speed,
            cache_dir: paths::audio_cache(),
            engine_dir: paths::engine_dir(kind),
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

/// Update the `[tts]` engine + voice lines in config.toml in place, preserving comments
/// and all other sections. Used by the runtime settings overlay to switch engines.
pub fn update_tts(engine: &str, voice: &str) -> Result<()> {
    let path = paths::config_file();
    let content = std::fs::read_to_string(&path)?;
    let mut in_tts = false;
    let mut found_engine = false;
    let mut found_voice = false;
    let mut out = String::with_capacity(content.len());
    // Match on the trimmed key before '=' — `starts_with("engine")` would miss a
    // legally-indented `  engine = …` and false-match any future `engine_*` key.
    let key_of = |line: &str| -> Option<String> {
        let t = line.trim_start();
        t.split_once('=')
            .map(|(k, _)| k.trim().to_string())
            .filter(|k| !k.is_empty() && !t.starts_with('#'))
    };
    for line in content.lines() {
        if line.trim_start().starts_with('[') {
            in_tts = line.trim() == "[tts]";
        }
        let key = key_of(line);
        if in_tts && key.as_deref() == Some("engine") {
            out.push_str(&format!("engine = \"{engine}\"\n"));
            found_engine = true;
        } else if in_tts && key.as_deref() == Some("voice") {
            out.push_str(&format!("voice = \"{voice}\"\n"));
            found_voice = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found_engine || !found_voice {
        bail!("config.toml [tts] section missing engine/voice lines");
    }
    std::fs::write(&path, out)?;
    Ok(())
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
# engine = kokoro | matcha | piper（运行时按 s 打开设置切换）
# kokoro 音色: af af_bella af_nicole af_sarah af_sky am_adam am_michael
#              bf_emma bf_isabella bm_george bm_lewis
engine = "kokoro"
voice = "af"
speed = 1.0

[a11y]
# reduced_motion = true 时跳过所有动画(星火接线弧光/评分反馈/卡片切换/morpheme 错峰)
reduced_motion = false
"#;
