//! The launcher's look, taken from the game's own interface.
//!
//! Sampled from the character creator: teal header bars, a parchment panel, brown ink for
//! body text, orange for the player's own name, and a lime button for the action that moves
//! you forward. Matching it costs nothing and makes the launcher read as part of the game
//! rather than as a tool that happens to start it.
//!
//! Colours only. Nothing here decides anything, which is why there is no test beside it.

use eframe::egui::{Color32, CornerRadius, Stroke};

/// The panel the game draws its menus on.
pub const PARCHMENT: Color32 = Color32::from_rgb(0xE9, 0xE5, 0xC9);
/// Its shaded edge.
pub const PARCHMENT_EDGE: Color32 = Color32::from_rgb(0xC9, 0xC4, 0xA0);
/// Behind the panel: the sky the creator floats in.
pub const SKY: Color32 = Color32::from_rgb(0x7E, 0xC8, 0xE8);

/// Section headers.
pub const TEAL: Color32 = Color32::from_rgb(0x4F, 0xB8, 0xB0);
pub const TEAL_DARK: Color32 = Color32::from_rgb(0x37, 0x91, 0x8B);

/// "Continue" in the creator, "Play" here.
pub const GREEN: Color32 = Color32::from_rgb(0x8B, 0xC5, 0x3F);
pub const GREEN_DARK: Color32 = Color32::from_rgb(0x6F, 0xA3, 0x2E);

/// The player's own name is orange in the creator.
pub const ORANGE: Color32 = Color32::from_rgb(0xE0, 0x8A, 0x3C);

/// Body text: brown rather than black, as the game draws it.
pub const INK: Color32 = Color32::from_rgb(0x4A, 0x46, 0x36);
/// Secondary text.
pub const INK_MUTED: Color32 = Color32::from_rgb(0x8A, 0x84, 0x6C);

/// Something the player needs to see. Red would be alarming for "no database yet".
pub const WARNING: Color32 = Color32::from_rgb(0xC4, 0x5A, 0x28);

pub const ROUND: CornerRadius = CornerRadius::same(4);

pub fn edge() -> Stroke {
    Stroke::new(1.0, PARCHMENT_EDGE)
}
