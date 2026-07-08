//! First-run setup — a calm, three-step ritual: bind an earphone (the emotional
//! center: silence is the default at the office), add a DeepSeek key, fetch the
//! voice model. Styled in the deep-water palette; falls back to a template when
//! stdin isn't a terminal (CI / piped).

use std::io::{Read, Write};

use anyhow::Result;

use crate::audio::coreaudio;
use crate::paths;

// ── deep-water palette (ANSI truecolor) ──
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

/// Run the wizard, returning nothing — it writes ~/.tuna/config.toml directly.
pub fn run() -> Result<()> {
    banner();
    let needle = step_earphone();
    let key = step_key();
    step_model();
    write_config(&needle, &key)?;
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

fn banner() {
    println!();
    println!(
        "  {} {}   {}",
        paint(FOAM, &bold("tuna")),
        paint(TEAL, "·"),
        paint(MUTED, "词根推导终端")
    );
    println!(
        "  {}",
        paint(MUTED, "词汇不是要存储的事实，是要推导的公式。")
    );
    println!("\n  {}\n", paint(MUTED, "首次运行 · 三步设置 ────────────────"));
}

fn step_earphone() -> String {
    println!("  {} {}", paint(TEAL, &bold("①")), bold("绑定耳机"));
    println!(
        "     {}",
        paint(MUTED, "只有它连着时 tuna 才发声。办公室里，静默是默认。")
    );

    let devices = coreaudio::enumerate().unwrap_or_default();
    let bt: Vec<&coreaudio::CoreAudioDevice> = devices
        .iter()
        .filter(|d| d.is_output() && d.is_bluetooth())
        .collect();

    if bt.is_empty() {
        println!(
            "     {}",
            paint(MUTED, "（暂未检测到蓝牙耳机——连上后重开也行，或先输入名字）")
        );
        let s = prompt(&format!(
            "     {} ",
            paint(TEAL, "▸ 耳机名字子串（回车用 airpods）:")
        ));
        return if s.is_empty() { "airpods".to_string() } else { s };
    }

    println!("     {}", paint(MUTED, "选一副绑定:"));
    for (i, d) in bt.iter().enumerate() {
        println!(
            "       {}  {}",
            paint(AMBER, &format!("{}", i + 1)),
            paint(FOAM, &d.name)
        );
    }
    let pick = prompt(&format!(
        "     {} ",
        paint(TEAL, "▸ 输入编号（回车用 1）:")
    ));
    let idx = pick.parse::<usize>().ok().filter(|n| *n >= 1 && *n <= bt.len());
    let chosen = &bt[idx.map(|n| n - 1).unwrap_or(0)];
    println!(
        "     {} {}",
        paint(GREEN, "✓ 已绑定"),
        paint(FOAM, &chosen.name)
    );
    chosen.name.clone()
}

fn step_key() -> String {
    println!("\n  {} {}", paint(TEAL, &bold("②")), bold("DeepSeek 密钥"));
    println!(
        "     {}",
        paint(
            MUTED,
            "用于苏格拉底辨析与点评你的推理。学习本身离线可用——可留空、以后填。"
        )
    );
    let key = prompt(&format!("     {} ", paint(TEAL, "▸ 粘贴密钥（回车跳过）:")));
    if key.is_empty() {
        println!("     {}", paint(MUTED, "· 跳过——之后可在 ~/.tuna/config.toml 补上"));
    } else {
        println!("     {}", paint(GREEN, "✓ 已记录"));
    }
    key
}

/// The two Kokoro assets tuna needs to speak: the quantized voice model + the voice
/// style pack. Same files whether inference runs on ort or elsewhere.
const MODEL_BASE: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";

fn step_model() {
    println!("\n  {} {}", paint(TEAL, &bold("③")), bold("发音模型（Kokoro）"));
    println!(
        "     {}",
        paint(MUTED, "本地 TTS，约 110MB。下齐才进入学习——之后按 Space 即刻发声，无需联网。")
    );
    let files = [
        ("kokoro-v1.0.int8.onnx", paths::kokoro_model()),
        ("voices-v1.0.bin", paths::kokoro_voices()),
    ];
    if files.iter().all(|(_, p)| p.exists()) {
        println!("     {}", paint(GREEN, "✓ 已就位，跳过"));
        return;
    }
    for (name, dst) in &files {
        if dst.exists() {
            continue;
        }
        loop {
            match download_with_progress(&format!("{MODEL_BASE}/{name}"), dst, name) {
                Ok(()) => break,
                Err(e) => {
                    println!("\n     {}", paint(CORAL, &format!("· 下载失败：{e}")));
                    let again = prompt(&format!(
                        "     {} ",
                        paint(TEAL, "▸ 重试？(y / 回车跳过，首次按 Space 时再下):")
                    ));
                    if !again.eq_ignore_ascii_case("y") {
                        println!(
                            "     {}",
                            paint(MUTED, "· 跳过——发音将在首次按 Space 时补下")
                        );
                        return;
                    }
                }
            }
        }
    }
    println!("     {}", paint(GREEN, "✓ 模型就位"));
}

/// Stream a URL to `dest` with a live, deep-water progress bar — pure Rust (reqwest),
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

fn write_config(needle: &str, key: &str) -> Result<()> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let toml = format!(
        "# tuna 配置 · ~/.tuna/config.toml（由首次设置生成，可随时手改）\n\
         # 也可用环境变量 DEEPSEEK_API_KEY 覆盖密钥。\n\n\
         [deepseek]\n\
         api_key = \"{}\"\n\
         base_url = \"https://api.deepseek.com\"\n\
         enrich_model = \"deepseek-v4-flash\"\n\
         chat_model = \"deepseek-v4-pro\"\n\n\
         [gate]\n\
         # 绑定耳机的名字子串（只在连着它时才发声）\n\
         needle = \"{}\"\n\n\
         [tts]\n\
         voice = \"af_heart\"\n\
         speed = 1.0\n",
        esc(key),
        esc(needle),
    );
    std::fs::write(paths::config_file(), toml)?;
    Ok(())
}
