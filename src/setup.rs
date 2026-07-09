//! First-run setup: bind an earphone, add a DeepSeek key, fetch a voice model.
//! Safe to re-run via `tuna setup` (existing values are pre-filled as defaults).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::audio::probe::{self, DeviceInfo};
use crate::audio::tts::{TtsEngineKind, from_kind};
use crate::config::Config;
use crate::paths;

// ── palette (ANSI truecolor) ──
const TEAL: (u8, u8, u8) = (52, 211, 194);
const AMBER: (u8, u8, u8) = (236, 179, 94);
const CORAL: (u8, u8, u8) = (237, 110, 92);
const FOAM: (u8, u8, u8) = (233, 239, 243);
const MUTED: (u8, u8, u8) = (110, 135, 152);
const GREEN: (u8, u8, u8) = (87, 192, 139);

fn paint(rgb: (u8, u8, u8), s: &str) -> String {
    format!("\x1b[38;2;{};{};{}m{s}\x1b[0m", rgb.0, rgb.1, rgb.2)
}
fn bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}

fn readline() -> String {
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok();
    s.trim().to_string()
}
fn prompt(p: &str) -> String {
    print!("{p}");
    std::io::stdout().flush().ok();
    readline()
}

/// Run the wizard. Loads existing config (if any) so re-runs keep your key and
/// earphone binding as defaults: just press Enter to keep each value.
pub fn run() -> Result<()> {
    let existing = Config::load().ok();
    let cur_needle = existing.as_ref().map(|c| c.gate.needle.as_str());
    let cur_key = existing.as_ref().map(|c| c.deepseek.api_key.as_str());
    let is_rerun = existing.is_some();
    banner(is_rerun);
    let needle = step_earphone(cur_needle);
    let key = step_key(cur_key);
    let (engine, voice) = step_engine();
    init_config(&needle, &key, &engine, &voice)?;
    Ok(())
}

/// The closing line, printed after the deck is built.
pub fn ready(words: usize, enriched: usize) {
    println!(
        "\n  {}  {}\n",
        paint(GREEN, &bold("✓ 就绪")),
        paint(
            MUTED,
            &format!("{words} 词就位 · {enriched} 已词源接地 · 开学")
        )
    );
}

fn banner(is_rerun: bool) {
    println!();
    println!(
        "  {} {}   {}",
        paint(FOAM, &bold("tuna")),
        paint(TEAL, "·"),
        paint(MUTED, "词根推导终端")
    );
    if is_rerun {
        println!(
            "\n  {}\n",
            paint(MUTED, "设置向导 · 回车保留当前值 ────────")
        );
    } else {
        println!("\n  {}\n", paint(MUTED, "首次运行 · 三步设置 ────────"));
    }
}

fn step_earphone(current: Option<&str>) -> String {
    println!("  {} {}", paint(TEAL, &bold("①")), bold("绑定耳机"));
    let cur = current.unwrap_or("");
    let hint = if cur.is_empty() {
        "只有它连着时 tuna 才发声。"
    } else {
        &format!("当前绑定「{cur}」，回车保留。")
    };
    println!("     {}", paint(MUTED, hint));

    let devices = probe::current_probe().enumerate().unwrap_or_default();
    let candidates: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| d.is_output() && gate_candidate(d))
        .collect();

    if candidates.is_empty() {
        println!(
            "     {}",
            paint(
                MUTED,
                "（暂未检测到合适的输出设备，连上后重开或先输入名字）"
            )
        );
        let default = if cur.is_empty() { "airpods" } else { cur };
        let s = prompt(&format!(
            "     {} ",
            paint(TEAL, &format!("▸ 耳机名字子串（回车用 {default}）:"))
        ));
        return if s.is_empty() { default.to_string() } else { s };
    }

    println!("     {}", paint(MUTED, "选一副绑定:"));
    #[cfg(not(target_os = "macos"))]
    {
        println!(
            "     {}",
            paint(
                MUTED,
                "（非 macOS：ALSA/WASAPI 设备名可能随重启漂移，如绑定失效请重跑 setup）"
            )
        );
    }
    // Pre-select the candidate matching the current needle, if any.
    let preselect = candidates
        .iter()
        .position(|d| cur.is_empty() || d.name.to_lowercase().contains(&cur.to_lowercase()))
        .unwrap_or(0);
    for (i, d) in candidates.iter().enumerate() {
        let marker = if i == preselect { " ← 当前" } else { "" };
        println!(
            "       {}  {}{}",
            paint(AMBER, &format!("{}", i + 1)),
            paint(FOAM, &d.name),
            paint(MUTED, marker)
        );
    }
    let default_pick = (preselect + 1).to_string();
    let pick = prompt(&format!(
        "     {} ",
        paint(TEAL, &format!("▸ 输入编号（回车用 {default_pick}）:"))
    ));
    let idx = pick
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= candidates.len());
    let chosen = &candidates[idx.map(|n| n - 1).unwrap_or(preselect)];
    println!(
        "     {} {}",
        paint(GREEN, "✓ 已绑定"),
        paint(FOAM, &chosen.name)
    );
    chosen.name.clone()
}

/// Whether a device is a candidate for the earphone gate. On macOS we filter to
/// bluetooth-class devices (the AirPods use case); elsewhere we accept any output
/// device because cpal 0.17 doesn't expose transport metadata.
fn gate_candidate(d: &DeviceInfo) -> bool {
    #[cfg(target_os = "macos")]
    {
        d.is_bluetooth()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = d;
        true
    }
}

fn step_key(current: Option<&str>) -> String {
    println!("\n  {} {}", paint(TEAL, &bold("②")), bold("DeepSeek 密钥"));
    let cur = current.unwrap_or("");
    if cur.is_empty() {
        println!(
            "     {}",
            paint(MUTED, "用于苏格拉底辨析。学习本身离线可用，可留空以后填。")
        );
    } else {
        println!("     {}", paint(MUTED, "已设置，回车保留当前密钥。"));
    }
    let key = prompt(&format!(
        "     {} ",
        paint(TEAL, "▸ 粘贴密钥（回车跳过/保留）:")
    ));
    if key.is_empty() {
        if cur.is_empty() {
            println!(
                "     {}",
                paint(MUTED, "· 跳过，之后可在 ~/.tuna/config.toml 补上")
            );
            String::new()
        } else {
            println!("     {}", paint(GREEN, "✓ 保留当前密钥"));
            cur.to_string()
        }
    } else {
        println!("     {}", paint(GREEN, "✓ 已记录"));
        key
    }
}

/// The engine picker: list Kokoro/Matcha/Piper with footprints + blurbs, let the
/// user choose, migrate any stale ort-pipeline files, then download + extract the
/// chosen engine's files. Returns `(engine_id, default_voice_id)` for init_config.
fn step_engine() -> (String, String) {
    println!("\n  {} {}", paint(TEAL, &bold("③")), bold("发音引擎"));
    println!(
        "     {}",
        paint(
            MUTED,
            "本地 TTS，三个引擎任选。下齐才进入学习，之后按 Space 即刻发声。"
        )
    );

    let kinds = TtsEngineKind::all();
    for (i, kind) in kinds.iter().enumerate() {
        let eng = from_kind(*kind);
        let tag = if i == 0 { "  推荐" } else { "" };
        println!(
            "       {}  {} · {} · ~{}MB{}",
            paint(AMBER, &format!("{}", i + 1)),
            paint(FOAM, &bold(kind.id())),
            eng.blurb(),
            eng.footprint_mb(),
            paint(MUTED, tag)
        );
    }

    let pick = prompt(&format!("     {} ", paint(TEAL, "▸ 输入编号（回车用 1）:")));
    let idx = pick
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= kinds.len());
    let chosen = kinds[idx.map(|n| n - 1).unwrap_or(0)];
    let eng = from_kind(chosen);
    let voice = eng.default_voice().id;
    println!(
        "     {} {}",
        paint(GREEN, "✓ 已选"),
        paint(FOAM, &format!("{} · {}", chosen.id(), eng.blurb()))
    );

    migrate_old_files();

    let engine_dir = paths::engine_dir(chosen);
    if eng.models_present(&engine_dir) {
        println!("     {}", paint(GREEN, "✓ 模型已就位，跳过"));
        return (chosen.id().to_string(), voice);
    }

    std::fs::create_dir_all(&engine_dir).ok();
    for dl in eng.downloads() {
        loop {
            let is_tarball = dl.dest.extension().map(|e| e == "bz2").unwrap_or(false);
            let result = if is_tarball {
                download_and_extract(&dl, &engine_dir)
            } else {
                let dst = engine_dir.join(&dl.dest);
                std::fs::create_dir_all(dst.parent().unwrap()).ok();
                download_with_progress(&dl.url, &dst, &dl.label)
            };
            match result {
                Ok(()) => break,
                Err(e) => {
                    println!("\n     {}", paint(CORAL, &format!("· 下载失败：{e}")));
                    let again = prompt(&format!(
                        "     {} ",
                        paint(TEAL, "▸ 重试？(y / 回车跳过，之后可重跑 tuna setup 补下):")
                    ));
                    if !again.eq_ignore_ascii_case("y") {
                        println!("     {}", paint(MUTED, "· 跳过，之后运行 tuna setup 补下"));
                        return (chosen.id().to_string(), voice);
                    }
                }
            }
        }
    }
    println!("     {}", paint(GREEN, "✓ 模型就位"));
    (chosen.id().to_string(), voice)
}

/// Download a `.tar.bz2` to a temp file and extract it into `engine_dir`, then clean up.
fn download_and_extract(dl: &crate::audio::tts::Download, engine_dir: &Path) -> Result<()> {
    let archive = engine_dir.join(&dl.dest);
    download_with_progress(&dl.url, &archive, &dl.label)?;
    extract_tar_bz2(&archive, engine_dir)?;
    std::fs::remove_file(&archive)?;
    Ok(())
}

/// Pure-Rust `.tar.bz2` extraction — no system `tar` dependency (Windows-safe).
fn extract_tar_bz2(archive: &Path, dest_dir: &Path) -> Result<()> {
    let f = std::fs::File::open(archive)?;
    let bz = bzip2::read::BzDecoder::new(f);
    let mut ar = tar::Archive::new(bz);
    ar.unpack(dest_dir)?;
    Ok(())
}

/// Detect leftover ort-pipeline files (old thewh1teagle Kokoro) at the tts/ root and
/// offer to purge them before downloading the sherpa version. The PIPELINE_VERSION bump
/// already invalidates the old audio cache, so we clear it here too.
fn migrate_old_files() {
    let tts_dir = paths::tts_dir();
    // Old layout: model + voices sitting directly under tts/, not in a kokoro/ subdir.
    let stale: Vec<PathBuf> = ["kokoro-v1.0.int8.onnx", "voices-v1.0.bin"]
        .iter()
        .map(|n| tts_dir.join(n))
        .filter(|p| p.exists())
        .collect();
    if stale.is_empty() {
        return;
    }
    println!(
        "\n     {}",
        paint(
            CORAL,
            "检测到旧版 Kokoro 文件（ort 管线），将删除并下载 sherpa 版。"
        )
    );
    for p in &stale {
        println!("       {}", paint(MUTED, &p.display().to_string()));
    }
    // Old PIPELINE_VERSION keys are dead — purge the audio cache too.
    if let Ok(entries) = std::fs::read_dir(paths::audio_cache()) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    for p in &stale {
        let _ = std::fs::remove_file(p);
    }
    println!("     {}", paint(GREEN, "✓ 旧文件已清理"));
}

/// Stream a URL to `dest` with a live progress bar. Pure Rust (reqwest),
/// no `curl` on the host. Writes to a `.part` file and renames on success so a killed
/// download never leaves a half-file that looks complete.
pub fn download_with_progress(url: &str, dest: &std::path::Path, label: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder().timeout(None).build()?;
    let mut resp = client.get(url).send()?.error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let tmp = dest.with_extension("part");
    let mut file = std::fs::File::create(&tmp)?;
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last = 0u64;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if done - last >= 512 * 1024 {
            render_bar(label, done, total.max(done));
            last = done;
        }
    }
    file.flush()?;
    drop(file);
    std::fs::rename(&tmp, dest)?;
    render_bar(label, done, done);
    println!("  {}", paint(GREEN, "✓"));
    Ok(())
}

fn render_bar(label: &str, done: u64, total: u64) {
    const WIDTH: usize = 26;
    let frac = if total > 0 {
        (done as f64 / total as f64).min(1.0)
    } else {
        0.0
    };
    let filled = (frac * WIDTH as f64).round() as usize;
    let mb = |b: u64| b as f64 / 1_048_576.0;
    print!(
        "\r     {} {}{} {} {}",
        paint(FOAM, &format!("↓ {label:<22}")),
        paint(TEAL, &"━".repeat(filled)),
        paint(MUTED, &"╌".repeat(WIDTH.saturating_sub(filled))),
        paint(AMBER, &format!("{:>3.0}%", frac * 100.0)),
        paint(MUTED, &format!("{:.1}/{:.1} MB", mb(done), mb(total))),
    );
    std::io::stdout().flush().ok();
}

fn init_config(needle: &str, key: &str, engine: &str, voice: &str) -> Result<()> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let toml = format!(
        "# tuna 配置 · ~/.tuna/config.toml（由首次设置生成，可随时手改）\n\
         # 也可用环境变量 DEEPSEEK_API_KEY 覆盖密钥。\n\n\
         [deepseek]\n\
         api_key = \"{key_esc}\"\n\
         base_url = \"https://api.deepseek.com\"\n\
         enrich_model = \"deepseek-v4-flash\"\n\
         chat_model = \"deepseek-v4-pro\"\n\n\
         [gate]\n\
         # 绑定耳机的名字子串（只在连着它时才发声）\n\
         needle = \"{needle_esc}\"\n\n\
         [tts]\n\
         # engine = kokoro | matcha | piper（运行时按 s 打开设置切换）\n\
         engine = \"{engine}\"\n\
         voice = \"{voice}\"\n\
         speed = 1.0\n",
        key_esc = esc(key),
        needle_esc = esc(needle),
    );
    std::fs::write(paths::config_file(), toml)?;
    Ok(())
}
