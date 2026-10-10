//! `ClientUISettingsComponent` — what is bound to the hotbar, and which square is selected.
//!
//! Read from the client's reader, `UISettingsComponent_ReadSync` (`FUN_008d4480`); see
//! `documentations/ui-settings-and-hotbar.md`. The widths are read, not yet proven in front
//! of the client, which is why the player only carries this behind `SKYSAGA_UI_SETTINGS`.
//!
//! | local | parameter | encoding |
//! |---:|---|---|
//! | 0 | `hotbarslotresources` | list, default length 8, of [`HotbarSlot`] |
//! | 1 | `activeslot` | 3 bits, clamped to 0..7 |

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::crafting::ItemSpec;

/// The hotbar's squares, which is also the list's default length: the base constructor
/// (`FUN_008d3500`) builds exactly eight.
pub const HOTBAR_SQUARES: usize = 8;

/// `8 - NumBitsRequired8(7)` (`FUN_007f3da0`).
const ACTIVE_SLOT_BITS: u32 = 3;

/// One square: what each of its two hands names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotbarSlot {
    /// The leading word of an entry (`+0x00`). Every write the client makes stores 0; its
    /// meaning is not known.
    pub unknown: u32,

    /// Hand 0 at `+0x04`, hand 1 at `+0x20` (`UISettings_SetHotbarSlotSpec`, `FUN_008d3330`).
    pub hands: [ItemSpec; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSettingsComponent {
    pub hotbar: Vec<HotbarSlot>,
    pub active_slot: u32,
}

impl Default for UiSettingsComponent {
    fn default() -> Self {
        Self {
            hotbar: vec![HotbarSlot::default(); HOTBAR_SQUARES],
            active_slot: 0,
        }
    }
}

impl UiSettingsComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "hotbarslotresources" => self.write_hotbar(writer),
            "activeslot" => writer.write_bits_le(self.active_slot.min(7), ACTIVE_SLOT_BITS),
            _ => return false,
        }

        true
    }

    /// `WriteHotbarSlotList` (`FUN_008d3e80`).
    ///
    /// The count is ranged 8..8, so it takes no bits at all; what follows is the usual
    /// escape, clear when the list is exactly its default length.
    fn write_hotbar(&self, writer: &mut BitWriter) {
        if self.hotbar.len() == HOTBAR_SQUARES {
            writer.write_bit(false);
        } else {
            writer.write_bit(true);
            writer.write_u32(self.hotbar.len() as u32);
        }

        for slot in &self.hotbar {
            writer.write_u32(slot.unknown);

            for hand in &slot.hands {
                hand.encode(writer);
            }
        }
    }
}
