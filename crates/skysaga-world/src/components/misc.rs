//! The remaining small components, each with one parameter.

use skysaga_proto::bitstream::BitWriter;

use super::ranged_bits;

/// `ClientUseEntityComponent` — what this entity is currently using.
///
/// The chest-opening trigger is the *player's* `usingentityid`, not the chest's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UseEntityComponent {
    pub using_entity_id: u32,
}

impl UseEntityComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "usingentityid" => writer.write_u32(self.using_entity_id),
            _ => return false,
        }

        true
    }
}

/// `ClientCraftingDropSlotsComponent` — the crafting grid's contents.
///
/// The count encoding here has no count field for a short list: below the default of 2,
/// nothing is written at all, so an empty list is a **zero-bit** payload whose flag is still
/// set. At or above the default it writes an escape bit and a full 32-bit count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CraftingDropSlotsComponent {
    pub slots: Vec<u32>,
}

impl CraftingDropSlotsComponent {
    const DEFAULT_COUNT: usize = 2;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "craftingdropslots" => {
                if self.slots.len() >= Self::DEFAULT_COUNT {
                    writer.write_bit(true);
                    writer.write_u32(self.slots.len() as u32);
                }

                for slot in &self.slots {
                    writer.write_u32(*slot);
                }
            }

            _ => return false,
        }

        true
    }
}

/// `ClientFeatureUnlockComponent` — which features are locked.
///
/// The C# writes a fixed 31 zero bits and ignores its own list, so every feature reads as
/// unlocked. Reproduced exactly rather than "fixed": the width is what the client parses, and
/// changing it would shift every parameter after it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeatureUnlockComponent {
    /// Held but not transmitted — see the note above.
    pub locked: Vec<bool>,
}

impl FeatureUnlockComponent {
    /// One leading bit plus thirty entries.
    pub const SYNCED_BITS: usize = 1 + 30;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "featureislockedstatuslist" => {
                for _ in 0..Self::SYNCED_BITS {
                    writer.write_bit(false);
                }
            }

            _ => return false,
        }

        true
    }
}

/// `ClientMailBoxComponent` — the player's mail.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MailBoxComponent {
    pub mail: Vec<MailItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MailItem {
    pub subject: String,
    pub body: String,
    pub unknown: String,
    pub timestamp: u64,
    pub message_uuid: String,
    pub attachment_entity: u32,
    pub text_arguments: Vec<String>,
    pub flags: u8,
}

impl MailBoxComponent {
    const DEFAULT_COUNT: usize = 64;
    const TEXT_ARGUMENTS_DEFAULT: usize = 5;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "mailitemlist" => {
                write_count(writer, self.mail.len(), Self::DEFAULT_COUNT);

                for item in &self.mail {
                    writer.write_string(&item.subject);
                    writer.write_string(&item.body);
                    writer.write_string(&item.unknown);
                    writer.write_u64_le(item.timestamp);
                    writer.write_string(&item.message_uuid);
                    writer.write_u32(item.attachment_entity);
                    writer.write_u8(item.flags);

                    write_count(
                        writer,
                        item.text_arguments.len(),
                        Self::TEXT_ARGUMENTS_DEFAULT,
                    );

                    for argument in &item.text_arguments {
                        writer.write_string(argument);
                    }

                    // Two trailing flags the emulator always clears.
                    writer.write_bit(false);
                    writer.write_bit(false);
                }
            }

            _ => return false,
        }

        true
    }
}

/// `ClientWalletComponent` — currency balances.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WalletComponent {
    pub currencies: Vec<Currency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Currency {
    /// `CRC32` of the currency name.
    pub name_hash: Option<u32>,
    pub value: u32,
}

impl WalletComponent {
    const DEFAULT_COUNT: usize = 8;

    /// Values up to 1023 use a short 10-bit form; larger ones a full 32-bit one, selected by
    /// a leading bit.
    const SMALL_VALUE_MAX: u32 = 1023;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "currency" => {
                write_count(writer, self.currencies.len(), Self::DEFAULT_COUNT);

                for currency in &self.currencies {
                    writer.write_optional_u32(currency.name_hash);

                    if currency.value <= Self::SMALL_VALUE_MAX {
                        writer.write_bit(false);
                        writer.write_bits_le(currency.value, ranged_bits(Self::SMALL_VALUE_MAX));
                    } else {
                        writer.write_bit(true);
                        writer.write_bits_le(currency.value, ranged_bits(u32::MAX));
                    }
                }
            }

            _ => return false,
        }

        true
    }
}

/// The protocol's usual count-optimised list header.
///
/// `min(count, default)` in a ranged field, then — only when the list is at or over the
/// default — an escape bit and the real 32-bit count. Note the boundary: a list *at* the
/// default takes the escape path.
fn write_count(writer: &mut BitWriter, count: usize, default: usize) {
    writer.write_bits_le(count.min(default) as u32, ranged_bits(default as u32));

    if count < default {
        return;
    }

    writer.write_bit(true);
    writer.write_u32(count as u32);
}
