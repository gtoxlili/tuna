//! The deep-water instrument palette, from the design brief.

use ratatui::style::Color;

/// Derivation current — the primary accent (teal).
pub const CURRENT: Color = Color::Rgb(52, 211, 194);
/// "You already own this" — known morphemes / the ZH meaning you arrive at.
pub const AMBER: Color = Color::Rgb(236, 179, 94);
/// Confusable / warning edges.
pub const CORAL: Color = Color::Rgb(237, 110, 92);
/// Primary text.
pub const FOAM: Color = Color::Rgb(233, 239, 243);
/// Secondary text.
pub const FOAM_DIM: Color = Color::Rgb(167, 188, 200);
/// Labels / tertiary.
pub const MUTED: Color = Color::Rgb(110, 135, 152);
/// Panel background.
pub const SLATE: Color = Color::Rgb(19, 33, 45);
/// Deepest background.
pub const ABYSS: Color = Color::Rgb(10, 21, 32);
/// "Good" grade / success.
pub const GREEN: Color = Color::Rgb(87, 192, 139);
