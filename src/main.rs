//! tuna — a terminal instrument for deriving 考研 English vocabulary.
//!
//! M0 milestone: prove the earphone gate on real hardware. Everything else in the
//! product depends on the guarantee validated here — that audio can be routed to a
//! bound earphone and *only* there, and that its absence means silence.

mod audio;
mod config;
mod data;
mod llm;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};

use audio::coreaudio;
use audio::player::{self, RoutedPlayer};
use data::Deck;

#[derive(Parser)]
#[command(name = "tuna", version, about = "考研英语 · 词根推导终端")]
struct Cli {
    /// No subcommand starts a study session.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every CoreAudio device with its UID, transport, and output-stream count.
    Probe,
    /// Play a test chime routed ONLY to the bound earphone. Silent (fail-closed) if absent.
    GateTest {
        /// Case-insensitive name substring of the earphone to bind (e.g. "airpods").
        #[arg(default_value = "airpods")]
        needle: String,
    },
    /// Build the study deck from ECDICT: every 考研-tagged word + a fresh FSRS card.
    BuildDeck {
        /// ECDICT SQLite database (download separately).
        #[arg(long, default_value = "data/stardict.db")]
        ecdict: PathBuf,
        /// Output deck path.
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
    },
    /// Show deck statistics (word count, new/introduced/due).
    DeckInfo {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
    },
    /// Start a study session (this is also the default with no subcommand).
    Study {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
    },
    /// Render the study screen to text (both card stages) for verification.
    RenderPreview {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
        /// Preview a specific word (else the queue front).
        #[arg(long)]
        word: Option<String>,
    },
    /// Pre-synthesize pronunciation audio (words + enriched examples) via Kokoro.
    Synth {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
        /// How many top-frequency words to synthesize.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Also synthesize each enriched word's example sentences.
        #[arg(long, default_value_t = true)]
        examples: bool,
    },
    /// Enrich words with DeepSeek (morphemes, derivation, graph edges, examples).
    Enrich {
        #[arg(long, default_value = "data/tuna.db")]
        deck: PathBuf,
        /// How many not-yet-enriched words to process this run.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Enrich one specific word instead (repeatable), regardless of order.
        #[arg(long)]
        word: Vec<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        None => ui::run(&PathBuf::from("data/tuna.db")),
        Some(Cmd::Probe) => probe(),
        Some(Cmd::GateTest { needle }) => gate_test(&needle),
        Some(Cmd::BuildDeck { ecdict, deck }) => build_deck(&ecdict, &deck),
        Some(Cmd::DeckInfo { deck }) => deck_info(&deck),
        Some(Cmd::Study { deck }) => ui::run(&deck),
        Some(Cmd::RenderPreview { deck, word }) => ui::preview(&deck, word),
        Some(Cmd::Enrich { deck, limit, word }) => enrich(&deck, limit, word),
        Some(Cmd::Synth {
            deck,
            limit,
            examples,
        }) => synth(&deck, limit, examples),
    }
}

fn synth(deck_path: &std::path::Path, limit: usize, examples: bool) -> Result<()> {
    let cfg = config::Config::load()?;
    let tts = cfg.tts_engine();
    if !tts.models_present() {
        anyhow::bail!(
            "Kokoro model not found at {} — download it (see README).",
            tts.model.display()
        );
    }
    let deck = Deck::open(deck_path)?;
    let words = deck.top_words(limit)?;

    // The word itself (pronunciation) + optionally its enriched example sentences.
    let mut texts = Vec::new();
    for w in &words {
        texts.push(w.clone());
        if examples {
            if let Some(en) = deck.enrichment(w)? {
                for ex in en.examples.iter().take(2) {
                    if !ex.en.trim().is_empty() {
                        texts.push(ex.en.clone());
                    }
                }
            }
        }
    }
    println!(
        "\n  synthesizing up to {} clips ({} words + examples) with voice {} …\n",
        texts.len(),
        words.len(),
        cfg.tts.voice
    );
    let made = tts.synth_batch(&texts)?;
    println!(
        "\n  ✓ {made} new clip(s) → {} ({} total requested)\n",
        tts.cache_dir.display(),
        texts.len()
    );
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
    let s = deck.stats()?;
    println!("  ✓ {n} 考研 words imported → {}", deck_path.display());
    println!(
        "    words {} · cards {} · new {} · due now {}\n",
        s.words, s.cards, s.new, s.due_now
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
    let devices = coreaudio::enumerate()?;
    println!("\n  CoreAudio devices ({} total)\n", devices.len());
    println!(
        "  {:<3} {:<28} {:<14} {:>7} {:<7} {}",
        "", "name", "transport", "out", "default", "uid"
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
            d.uid,
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
            println!("    • {}  →  bind by uid: {}", d.name, d.uid);
        }
        println!();
    }
    Ok(())
}

fn gate_test(needle: &str) -> Result<()> {
    let devices = coreaudio::enumerate()?;
    let default_name = player::default_output_name().unwrap_or_else(|| "unknown".to_string());

    println!("\n  earphone gate · needle = \"{needle}\"");
    println!("  system default output : {default_name}");

    let Some(bound) = coreaudio::find_bound_output(&devices, needle) else {
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
    println!("  bound uid             : {}", bound.uid);

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
