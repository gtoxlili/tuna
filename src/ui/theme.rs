//! UI color palette: ink-black background with teal current and amber accents.

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
/// Labels / tertiary. Tuned to clear WCAG AA 4.5:1 against SLATE (was 110,135,152 ~4.0:1).
pub const MUTED: Color = Color::Rgb(130, 155, 172);
/// Panel background.
pub const SLATE: Color = Color::Rgb(19, 33, 45);
/// Deepest background.
pub const ABYSS: Color = Color::Rgb(10, 21, 32);
/// "Good" grade / success.
pub const GREEN: Color = Color::Rgb(87, 192, 139);
/// Row wash under the current speak target — ABYSS pulled ~12% toward CURRENT.
/// Soft enough to read as a highlight strip, not a block cursor.
pub const SPEAK_BG: Color = Color::Rgb(15, 44, 51);

// RGB components of ABYSS — used by the grade-flash tint math in render_card.
pub const ABYSS_R: u8 = 10;
pub const ABYSS_G: u8 = 21;
pub const ABYSS_B: u8 = 32;
