//! Cross-platform audio device probe — the honest source of truth for the earphone gate.
//!
//! macOS keeps its CoreAudio HAL path (UID + transport fourcc disambiguates AirPods
//! HFP/A2DP duplicates — cpal 0.17 can't). Linux/Windows use cpal's ALSA/WASAPI backends.
//! The trait abstracts *which* probe runs; `current_probe()` picks per target.

use anyhow::Result;

/// How a device reaches the host — drives the earphone-gate candidate filter and the
/// `probe` command's transport column. macOS fills this from CoreAudio fourcc; Linux
/// and Windows default to `Unknown` because cpal 0.17 doesn't expose transport metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // variants used only on some platforms (macOS fills most; Linux/Windows use Unknown)
pub enum Transport {
    Bluetooth,
    BluetoothLE,
    BuiltIn,
    Usb,
    AirPlay,
    Hdmi,
    DisplayPort,
    Aggregate,
    Virtual,
    Other,
    Unknown,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Bluetooth => "bluetooth",
            Transport::BluetoothLE => "bluetooth-le",
            Transport::BuiltIn => "built-in",
            Transport::Usb => "usb",
            Transport::AirPlay => "airplay",
            Transport::Hdmi => "hdmi",
            Transport::DisplayPort => "displayport",
            Transport::Aggregate => "aggregate",
            Transport::Virtual => "virtual",
            Transport::Other => "other",
            Transport::Unknown => "unknown",
        }
    }
}

/// A single audio device, platform-agnostic. `stable_id` is the macOS UID on macOS
/// (stable across reconnect, embeds the BT MAC); on Linux/Windows it currently falls
/// back to the device name (config stores the name; cpal's stable `id()` is a planned
/// upgrade), so binding by name is the honest contract there, with a drift warning
/// surfaced in the setup wizard.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub stable_id: String,
    pub name: String,
    pub transport: Transport,
    pub out_streams: usize,
    pub is_default_output: bool,
}

impl DeviceInfo {
    /// A device we can play through has at least one output stream.
    pub fn is_output(&self) -> bool {
        self.out_streams > 0
    }

    /// Bluetooth-class transports (BT, BT-LE) — the earphone-gate candidates. AirPlay
    /// is intentionally excluded to match the original CoreAudio behaviour, which only
    /// treated classic Bluetooth and Bluetooth-LE as gate candidates.
    pub fn is_bluetooth(&self) -> bool {
        matches!(
            self.transport,
            Transport::Bluetooth | Transport::BluetoothLE
        )
    }

    pub fn transport_label(&self) -> &'static str {
        self.transport.label()
    }

    /// Devices that exist regardless of whether any physical hardware is present —
    /// ALSA's virtual PCMs (`default`, `pulse`, `pipewire`, …). Binding one would
    /// hold the gate open forever and route audio to whatever the sound server
    /// currently picks — usually the speakers — the exact leak the gate exists to
    /// prevent. They are excluded from gate candidates and gate matching.
    pub fn is_ungateable(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            let lower = self.name.to_lowercase();
            let base = lower.split(':').next().unwrap_or(&lower);
            matches!(
                base,
                "default"
                    | "sysdefault"
                    | "pulse"
                    | "pipewire"
                    | "null"
                    | "jack"
                    | "oss"
                    | "dmix"
                    | "dsnoop"
                    | "samplerate"
                    | "speexrate"
                    | "upmix"
                    | "vdownmix"
                    | "lavrate"
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

/// Platform-abstracted device enumeration. Implementations live behind cfg gates.
pub trait AudioProbe {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>>;
}

/// The probe for the current target.
pub fn current_probe() -> Box<dyn AudioProbe> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacosProbe)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(CpalProbe)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(CpalProbe)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        compile_error!("tuna requires macOS, Linux, or Windows");
    }
}

// ── macOS: CoreAudio HAL (UID + transport fourcc) ─────────────────────────────

#[cfg(target_os = "macos")]
pub struct MacosProbe;

#[cfg(target_os = "macos")]
impl AudioProbe for MacosProbe {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        crate::audio::coreaudio::enumerate()
    }
}

// ── Linux / Windows: cpal ALSA / WASAPI ───────────────────────────────────────
//
// cpal 0.17 doesn't expose transport type on these targets. It DOES have a stable
// `Device::id()` (WASAPI endpoint id / ALSA PCM id) — binding still uses the display
// name because that's what config.toml stores and what the user recognizes; moving
// the binding to `id()` is a known improvement that needs a config-schema change
// (tracked in backlog). We treat every enumerated output device as a one-stream
// `Unknown`-transport output and let the user pick by name in the setup wizard —
// with a warning that ALSA names can drift across reboots, so rebinding may be needed.

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub struct CpalProbe;

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl AudioProbe for CpalProbe {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        let host = rodio::cpal::default_host();
        let default = host.default_output_device();
        let default_name = default.as_ref().and_then(|d| d.name().ok());
        let mut out = Vec::new();
        if let Ok(mut devs) = host.output_devices() {
            while let Some(d) = devs.next() {
                let name = d.name().unwrap_or_else(|_| "unknown".to_string());
                let is_default = default_name.as_ref() == Some(&name);
                out.push(DeviceInfo {
                    stable_id: name.clone(),
                    name,
                    transport: Transport::Unknown,
                    out_streams: 1,
                    is_default_output: is_default,
                });
            }
        }
        Ok(out)
    }
}

/// Find the bound earphone among output devices — the GATE policy, shared by the
/// study session's `poll_gate` and `gate-test`. Matched case-insensitively by name
/// substring, with two hard requirements on top:
///
/// - always-present virtual sinks (`is_ungateable`) never match — a gate that can't
///   close is not a gate;
/// - on macOS, only Bluetooth-class devices qualify. The transport is known there,
///   and a loose needle (say "air" hand-edited into config.toml) must not open the
///   gate on "MacBook Air扬声器". On Linux/Windows cpal exposes no transport, so
///   name matching stays the honest (weaker, documented) contract.
///
/// A Bluetooth match is preferred when several outputs share the needle — this is
/// what disambiguates an AirPods output from a same-named HFP input on macOS.
pub fn find_bound_output<'a>(devices: &'a [DeviceInfo], needle: &str) -> Option<&'a DeviceInfo> {
    let needle = needle.to_lowercase();
    let mut matches: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| {
            d.is_output() && !d.is_ungateable() && d.name.to_lowercase().contains(&needle)
        })
        .collect();
    #[cfg(target_os = "macos")]
    matches.retain(|d| d.is_bluetooth());
    matches.sort_by_key(|d| !d.is_bluetooth());
    matches.into_iter().next()
}

/// Permissive name lookup with no gate policy — for diagnostics (`gate-test` uses it
/// to explain WHY a device the gate refuses would otherwise have matched).
pub fn find_output_by_name<'a>(devices: &'a [DeviceInfo], needle: &str) -> Option<&'a DeviceInfo> {
    let needle = needle.to_lowercase();
    devices
        .iter()
        .find(|d| d.is_output() && d.name.to_lowercase().contains(&needle))
}
