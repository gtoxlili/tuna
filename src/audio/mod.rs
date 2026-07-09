//! Audio subsystem: device truth + a device-bound player = the earphone gate.
//!
//! `probe` is the platform-abstracted device enumeration (CoreAudio HAL on macOS,
//! cpal ALSA/WASAPI elsewhere). `coreaudio` is macOS-only and lives behind a cfg gate.
//! `player` and `tts` are cross-platform (cpal / sherpa-onnx).

#[cfg(target_os = "macos")]
pub mod coreaudio;
pub mod player;
pub mod probe;
pub mod tts;
