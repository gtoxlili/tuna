//! tuna — a terminal instrument for deriving 考研 English vocabulary.
//!
//! M0 milestone: prove the earphone gate on real hardware. Everything else in the
//! product depends on the guarantee validated here — that audio can be routed to a
//! bound earphone and *only* there, and that its absence means silence.

mod audio;

use anyhow::Result;
use clap::{Parser, Subcommand};

use audio::coreaudio;
use audio::player::{self, RoutedPlayer};

#[derive(Parser)]
#[command(name = "tuna", version, about = "考研英语 · 词根推导终端")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Probe => probe(),
        Cmd::GateTest { needle } => gate_test(&needle),
    }
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
