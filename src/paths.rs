//! Everything tuna needs lives under one directory: ~/.tuna. Same on any device,
//! nothing next to the binary. First run detects an empty ~/.tuna and bootstraps it.

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The single root: ~/.tuna (override with $TUNA_HOME).
pub fn root() -> PathBuf {
    std::env::var_os("TUNA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".tuna"))
}

pub fn config_file() -> PathBuf {
    root().join("config.toml")
}
pub fn deck_db() -> PathBuf {
    root().join("tuna.db")
}
pub fn audio_cache() -> PathBuf {
    root().join("cache").join("audio")
}
pub fn tts_dir() -> PathBuf {
    root().join("tts")
}
pub fn kokoro_model() -> PathBuf {
    tts_dir().join("kokoro-v1.0.int8.onnx")
}
pub fn kokoro_voices() -> PathBuf {
    tts_dir().join("voices-v1.0.bin")
}

/// Has tuna been set up here yet?
pub fn is_initialized() -> bool {
    deck_db().exists() && config_file().exists()
}

/// Create the directory tree (idempotent).
pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [root(), audio_cache(), tts_dir()] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}
