//! tuna — a terminal instrument for deriving 考研 English vocabulary.
//!
//! M0 milestone: prove the earphone gate on real hardware. Everything else in the
//! product depends on the guarantee validated here — that audio can be routed to a
//! bound earphone and *only* there, and that its absence means silence.

mod assets;
mod audio;
mod config;
mod data;
mod llm;
mod paths;
mod setup;
mod ui;

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};

use audio::player::{self, RoutedPlayer};
use audio::probe;
use data::Deck;

#[derive(Parser)]
#[command(name = "tuna", version, about = "考研英语 · 词根推导终端")]
struct Cli {
    /// No subcommand starts a study session; first run bootstraps ~/.tuna.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a study session (default). First run initializes ~/.tuna.
    Study,
    /// Re-run the setup wizard (rebind earphone, set key, download voice models).
    /// Use this when you skipped model download on first run, or want to switch engines.
    Setup,
    /// Socratic 辨析 of a word vs its confusables (needs a DeepSeek key).
    Ask { word: String },
    /// Deck statistics.
    DeckInfo,
    /// List audio devices (stable_id / transport / output-streams).
    Probe,
    /// Play a test chime routed ONLY to the bound earphone; silent if absent.
    GateTest {
        #[arg(default_value = "airpods")]
        needle: String,
    },

    // ── maintainer / dev commands (hidden) ──
    /// [maintainer] Build the dev deck from ECDICT.
    #[command(hide = true)]
    BuildDeck {
        #[arg(long, default_value = "data/stardict.db")]
        ecdict: PathBuf,
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
    },
    /// [maintainer] Export the dev deck to the committed assets/deck.jsonl.
    #[command(hide = true)]
    ExportDeck {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
        #[arg(long, default_value = "assets/deck.jsonl")]
        out: PathBuf,
    },
    /// [maintainer] Enrich words with DeepSeek into the dev deck.
    #[command(hide = true)]
    Enrich {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        word: Vec<String>,
    },
    /// [dev] Render the study screen to text for verification.
    #[command(hide = true)]
    RenderPreview {
        #[arg(long)]
        word: Option<String>,
    },
    /// [dev] Synthesize a word to a WAV via the sherpa-onnx engine (no playback).
    #[command(hide = true)]
    Synth {
        text: String,
        #[arg(long, default_value = "/tmp/tuna-synth.wav")]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        None | Some(Cmd::Study) => {
            ensure_ready()?;
            ui::run(&paths::deck_db())
        }
        Some(Cmd::Ask { word }) => {
            ensure_ready()?;
            ask_cmd(&word)
        }
        Some(Cmd::DeckInfo) => {
            ensure_ready()?;
            deck_info(&paths::deck_db())
        }
        Some(Cmd::Probe) => probe(),
        Some(Cmd::GateTest { needle }) => gate_test(&needle),
        Some(Cmd::Setup) => setup::run(),
        Some(Cmd::BuildDeck { ecdict, deck }) => build_deck(&ecdict, &deck),
        Some(Cmd::ExportDeck { deck, out }) => export_deck(&deck, &out),
        Some(Cmd::Enrich { deck, limit, word }) => enrich(&deck, limit, word),
        Some(Cmd::RenderPreview { word }) => {
            ensure_ready()?;
            ui::preview(&paths::deck_db(), word)
        }
        Some(Cmd::Synth { text, out }) => synth_probe(&text, &out),
    }
}

/// [dev] Synthesize a word through the sherpa-onnx engine and report clip stats —
/// proves the TTS pipeline end to end without needing playback.
fn synth_probe(text: &str, out: &std::path::Path) -> Result<()> {
    let cfg = config::Config::load()?;
    let tts = cfg.tts_engine();
    anyhow::ensure!(
        tts.models_present(),
        "TTS 模型未就位 under {} — run setup or set TUNA_HOME.",
        paths::tts_dir().display()
    );
    let mut session = audio::tts::start_session(&tts)?;
    let t0 = std::time::Instant::now();
    let (samples, sample_rate) = session.synth_raw(text, &tts.voice, tts.speed)?;
    let path = out
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("WAV path is not UTF-8: {}", out.display()))?;
    if !sherpa_onnx::write(path, &samples, sample_rate) {
        anyhow::bail!("sherpa write failed for {}", out.display());
    }
    let n = samples.len();
    let secs = n as f32 / sample_rate as f32;
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n.max(1) as f32).sqrt();
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!(
        "  ✓ synth {text:?} → {}\n    {} samples · {:.2}s @ {}Hz · rms {:.4} · peak {:.3}",
        out.display(),
        n,
        secs,
        sample_rate,
        rms,
        peak
    );
    println!("    首次合成耗时 {:.1}s（含图优化）", t0.elapsed().as_secs_f32());
    Ok(())
}

/// First run: create ~/.tuna, drop the config, build the DB from the embedded
/// assets (no ECDICT, no network). TTS runs on sherpa-onnx — statically linked C++ lib.
fn bootstrap() -> Result<()> {
    paths::ensure_dirs()?;

    // First run: the interactive wizard (bind earphone, key, voice model) when
    // stdout is a terminal; a plain template when piped (CI / non-interactive).
    let interactive = std::io::stdout().is_terminal();
    if !paths::config_file().exists() {
        if interactive {
            setup::run()?;
        } else {
            std::fs::write(paths::config_file(), config::TEMPLATE)?;
        }
    }

    let mut deck = Deck::open(&paths::deck_db())?;
    let n = deck.build_from_asset(assets::DECK)?;
    let enr = deck.load_enrichment_str(assets::ENRICHMENT)?;
    // Reunite cognate root-fragments (spect/spectāculum/spectate → spect) so the graph
    // links words that truly share a root. Deterministic + gloss-gated; assets untouched.
    deck.canonicalize_cognates()?;
    if interactive {
        setup::ready(n, enr);
    } else {
        println!(
            "  ✓ 初始化 {} — {n} 词 · {enr} 已精加工",
            paths::root().display()
        );
    }
    Ok(())
}

fn ensure_ready() -> Result<()> {
    if !paths::is_initialized() {
        bootstrap()?;
    } else {
        // Config exists, but the deck might be empty (partial bootstrap, corrupted
        // DB). Rebuild from the embedded asset so the user never lands on an empty
        // screen with no recovery path.
        let mut deck = Deck::open(&paths::deck_db())?;
        if deck.stats()?.cards == 0 {
            deck.build_from_asset(assets::DECK)?;
            deck.load_enrichment_str(assets::ENRICHMENT)?;
            deck.canonicalize_cognates()?;
        }
    }
    Ok(())
}

fn export_deck(deck: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let d = Deck::open(deck)?;
    let n = d.export_deck_jsonl(out)?;
    println!("  ✓ exported {n} words → {}", out.display());
    Ok(())
}

fn ask_cmd(word: &str) -> Result<()> {
    let cfg = config::Config::load()?;
    let key = cfg.require_key()?;
    let client = llm::DeepSeek::new(cfg.deepseek.base_url.clone(), key.to_string());
    let deck = Deck::open(&paths::deck_db())?;
    let context = match deck.enrichment(word)? {
        Some(en) => {
            let neighbours: Vec<String> = en
                .graph_edges
                .iter()
                .filter(|e| e.relation == "confusable" || e.relation == "synonym")
                .map(|e| e.target.clone())
                .collect();
            format!("词义: {}\n易混/近义: {}", en.gloss_zh, neighbours.join(", "))
        }
        None => String::new(),
    };
    println!("\n  苏格拉底 · {word}\n");
    let text = llm::socratic::socratic(&client, &cfg.deepseek.chat_model, word, &context)?;
    println!("{text}\n");
    Ok(())
}

fn enrich(deck_path: &std::path::Path, limit: usize, words_arg: Vec<String>) -> Result<()> {
    let cfg = config::Config::load()?;
    let key = cfg.require_key()?;
    let client = llm::DeepSeek::new(cfg.deepseek.base_url.clone(), key.to_string());
    let deck = Deck::open(deck_path)?;

    let words = if words_arg.is_empty() {
        deck.words_to_enrich(limit)?
    } else {
        words_arg
    };
    if words.is_empty() {
        println!("\n  nothing to enrich — every word is done.\n");
        return Ok(());
    }
    println!(
        "\n  enriching {} word(s) with {} …\n",
        words.len(),
        cfg.deepseek.enrich_model
    );

    let (mut prompt, mut cached, mut completion, mut ok) = (0u64, 0u64, 0u64, 0usize);
    for (i, w) in words.iter().enumerate() {
        let tag = format!("[{:>3}/{}] {:<18}", i + 1, words.len(), w);
        if !deck.has_word(w)? {
            println!("  {tag} ⤳ 跳过（不在考研牌组内）");
            continue;
        }
        match llm::enrich::enrich_word(&client, &cfg.deepseek.enrich_model, w, &[]) {
            Ok((e, raw, usage)) => {
                prompt += usage.prompt;
                cached += usage.cached;
                completion += usage.completion;
                if let Err(err) = deck.save_enrichment(&e, &raw) {
                    println!("  {tag} ✗ 存储失败: {err}");
                    continue;
                }
                ok += 1;
                let conf = if e.etymology_confidence.is_empty() {
                    "?"
                } else {
                    &e.etymology_confidence
                };
                let deriv: String = e.derivation_zh.chars().take(34).collect();
                println!(
                    "  {tag} {conf:<8} {}morph {}edge  {deriv}",
                    e.morphemes.len(),
                    e.graph_edges.len(),
                );
            }
            Err(err) => println!("  {tag} ✗ {err}"),
        }
    }
    println!(
        "\n  ✓ {ok}/{} enriched · tokens: prompt {prompt} (cached {cached}) + completion {completion}\n",
        words.len()
    );
    Ok(())
}

fn build_deck(ecdict: &std::path::Path, deck_path: &std::path::Path) -> Result<()> {
    println!("\n  building deck from {} …", ecdict.display());
    let mut deck = Deck::open(deck_path)?;
    let n = deck.build_from_ecdict(ecdict)?;
    let enr = deck.load_enrichment_asset(std::path::Path::new("assets/enrichment.jsonl"))?;
    let s = deck.stats()?;
    println!("  ✓ {n} 考研 words imported → {}", deck_path.display());
    println!(
        "    words {} · cards {} · new {} · due now {} · enriched {}\n",
        s.words, s.cards, s.new, s.due_now, enr
    );
    Ok(())
}

fn deck_info(deck_path: &std::path::Path) -> Result<()> {
    let deck = Deck::open(deck_path)?;
    let s = deck.stats()?;
    println!("\n  deck: {}", deck_path.display());
    println!("  ─────────────────────────────");
    println!("  words       {}", s.words);
    println!("  cards       {}", s.cards);
    println!("  new         {}", s.new);
    println!("  introduced  {}", s.introduced);
    println!("  due now     {}", s.due_now);

    // Show a few of the first cards queued so the pipeline is legible.
    let queue = deck.next_queue(Utc::now(), 6)?;
    if !queue.is_empty() {
        println!("\n  next up (frequency-ordered):");
        for c in &queue {
            if let Some(e) = deck.entry(&c.word)? {
                println!(
                    "    {:<16} {:<20} {}",
                    e.word,
                    e.phonetic,
                    e.translation.lines().next().unwrap_or("").trim()
                );
            }
        }
    }
    println!();
    Ok(())
}

fn probe() -> Result<()> {
    let devices = probe::current_probe().enumerate()?;
    println!("\n  audio devices ({} total)\n", devices.len());
    println!(
        "  {:<3} {:<28} {:<14} {:>7} {:<7} {}",
        "", "name", "transport", "out", "default", "stable_id"
    );
    println!("  {}", "─".repeat(88));
    for d in &devices {
        let marker = if d.is_default_output {
            "▶"
        } else if d.is_output() {
            "·"
        } else {
            " "
        };
        println!(
            "  {:<3} {:<28} {:<14} {:>7} {:<7} {}",
            marker,
            truncate(&d.name, 28),
            d.transport_label(),
            d.out_streams,
            if d.is_default_output { "yes" } else { "" },
            d.stable_id,
        );
    }
    println!("\n  ▶ = system default output   · = other output device\n");

    // Surface the same-name duplicate reality explicitly, since it decides the design.
    let bt_outputs: Vec<_> = devices
        .iter()
        .filter(|d| d.is_output() && d.is_bluetooth())
        .collect();
    if !bt_outputs.is_empty() {
        println!("  bluetooth output devices (gate candidates):");
        for d in bt_outputs {
            println!("    • {}  →  bind by stable_id: {}", d.name, d.stable_id);
        }
        println!();
    }
    Ok(())
}

fn gate_test(needle: &str) -> Result<()> {
    let devices = probe::current_probe().enumerate()?;
    let default_name = player::default_output_name().unwrap_or_else(|| "unknown".to_string());

    println!("\n  earphone gate · needle = \"{needle}\"");
    println!("  system default output : {default_name}");

    let Some(bound) = probe::find_bound_output(&devices, needle) else {
        println!("\n  ✗ GATE CLOSED — no output device matches \"{needle}\".");
        println!("    → holding silence (fail-closed). Nothing played through any speaker.\n");
        return Ok(());
    };

    println!(
        "  bound earphone        : {}  [{}, out-streams {}]",
        bound.name,
        bound.transport_label(),
        bound.out_streams
    );
    println!("  bound stable_id       : {}", bound.stable_id);

    if !bound.is_bluetooth() {
        println!(
            "\n  ⚠ note: matched device is not bluetooth ({}). Continuing anyway for the test.",
            bound.transport_label()
        );
    }

    // Open the stream on the bound cpal device (matched by the same name). If cpal
    // can't find it as an output device, treat as gate-closed rather than falling back.
    let Some(cpal_device) = player::find_output_device(&bound.name) else {
        println!("\n  ✗ GATE CLOSED — CoreAudio saw the device but cpal could not open it as output.");
        println!("    → holding silence (fail-closed).\n");
        return Ok(());
    };

    let routed = RoutedPlayer::open(cpal_device)?;
    let diverged = routed.device_name != default_name;
    println!("\n  ▶ routing a chime to: {}", routed.device_name);
    if diverged {
        println!(
            "    (system default is \"{default_name}\" — so this proves audio goes to the\n     earphone even when it is NOT the default output. No speaker leak.)"
        );
    } else {
        println!(
            "    (this device is also the current default. To fully prove the gate, set the\n     Mac output to the built-in speakers and run this again — the chime must stay in the earphone.)"
        );
    }
    routed.play_test_chime();
    println!("\n  ✓ chime finished. Confirm you heard it ONLY in the earphone.\n");
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
