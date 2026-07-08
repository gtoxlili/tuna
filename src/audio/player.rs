//! The routed player: audio physically opens on a *chosen* output device, never
//! the system default. If the bound earphone is absent we simply hold no stream —
//! there is no speaker stream to leak from. That is the earphone gate.

use std::time::Duration;

use anyhow::{Context, Result};
use rodio::cpal::{
    self,
    traits::{DeviceTrait, HostTrait},
};
use rodio::source::{SineWave, Source};
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

/// A playback stream bound to one specific output device. Dropping it ends playback
/// and disposes the OS sink — so a mid-session disconnect becomes instant silence.
pub struct RoutedPlayer {
    // Field order matters for drop order, but both are torn down together on drop;
    // the sink is kept alive explicitly because dropping it stops the stream.
    _sink: MixerDeviceSink,
    player: Player,
    pub device_name: String,
}

/// cpal 0.17 deprecated `name()` in favour of a stable `id()` + rich `description()`.
/// For M0 we match CoreAudio's name, so `name()` is what we want; production binding
/// will likely move to the stable `id()`. Kept in one place so the migration is one edit.
#[allow(deprecated)]
fn device_name(device: &cpal::Device) -> Option<String> {
    device.name().ok()
}

impl RoutedPlayer {
    /// Open a stream bound to `device`.
    pub fn open(device: cpal::Device) -> Result<Self> {
        let device_name = device_name(&device).unwrap_or_else(|| "unknown".to_string());
        let sink = DeviceSinkBuilder::from_device(device)
            .context("could not build sink for the bound device")?
            .open_stream()
            .context("could not open an audio stream on the bound device")?;
        let player = Player::connect_new(sink.mixer());
        Ok(Self {
            _sink: sink,
            player,
            device_name,
        })
    }

    /// A short, unobtrusive confirmation chime (a rising two-note motif) so the
    /// M0 spike can be heard. Blocks until the chime finishes.
    pub fn play_test_chime(&self) {
        for (freq, ms) in [(587.33_f32, 160u64), (880.0, 260)] {
            let note = SineWave::new(freq)
                .take_duration(Duration::from_millis(ms))
                .amplify(0.18)
                .fade_in(Duration::from_millis(20));
            self.player.append(note);
        }
        self.player.sleep_until_end();
    }
}

/// The system default output device's name — shown so the spike can prove the
/// routed device and the default can diverge (e.g. default = speakers, routed = AirPods).
pub fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| device_name(&d))
}

/// Find a cpal *output* device by case-insensitive name substring.
pub fn find_output_device(needle: &str) -> Option<cpal::Device> {
    let needle = needle.to_lowercase();
    let host = cpal::default_host();
    host.output_devices().ok()?.find(|d| {
        device_name(d)
            .map(|n| n.to_lowercase().contains(&needle))
            .unwrap_or(false)
    })
}
