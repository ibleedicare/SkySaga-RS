//! Wire types shared by packets and components.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// `ItemSpec` — identifies an item and its materials.
///
/// A default one is 171 bits, which is how it was confirmed: every other parameter of the
/// captured Sheep accounts for 135 bits of a 306-bit payload, and the remainder is exactly
/// this.
///
/// ```text
/// name_hash      optional u32              1 + 32
/// material count 3 bits, ranged max 4
/// escape         1 bit + 32-bit count      only when the list is at or over the default of 4
/// materials      count x optional u32      33 each when present
/// teach_item     optional u32              1 + 32
/// uuid           string
/// ```
///
/// Note the count encoding: a list *at* the default length takes the escape path, not the
/// short one. The condition is `count < default`, so exactly 4 materials writes the 3-bit
/// count, then the escape bit, then the full 32-bit count again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSpec {
    pub name_hash: Option<u32>,
    pub materials: Vec<Option<u32>>,
    pub teach_item: Option<u32>,
    pub uuid: String,
}

impl Default for ItemSpec {
    fn default() -> Self {
        Self {
            name_hash: None,
            materials: vec![Some(0); Self::DEFAULT_MATERIALS],
            teach_item: None,
            uuid: String::new(),
        }
    }
}

impl ItemSpec {
    pub const DEFAULT_MATERIALS: usize = 4;

    /// Width of a default `ItemSpec`: 1 + 3 + 1 + 32 + 4x33 + 1 + 1.
    pub const DEFAULT_BITS: usize = 1 + 3 + 1 + 32 + 4 * 33 + 1 + 1;

    const COUNT_BITS: u32 = 32 - (Self::DEFAULT_MATERIALS as u32).leading_zeros();

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_optional_u32(self.name_hash);

        if self.materials.len() < Self::DEFAULT_MATERIALS {
            writer.write_bits_le(self.materials.len() as u32, Self::COUNT_BITS);
        } else {
            writer.write_bits_le(Self::DEFAULT_MATERIALS as u32, Self::COUNT_BITS);
            writer.write_bit(true);
            writer.write_u32(self.materials.len() as u32);
        }

        for material in &self.materials {
            writer.write_optional_u32(*material);
        }

        writer.write_optional_u32(self.teach_item);
        writer.write_string(&self.uuid);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let name_hash = reader.read_optional_u32()?;

        let short = reader.read_bits_le(Self::COUNT_BITS)? as usize;

        let count = if short >= Self::DEFAULT_MATERIALS {
            // The escape path: the short field is saturated and the real count follows.
            let _escape = reader.read_bit()?;
            reader.read_u32()? as usize
        } else {
            short
        };

        let mut materials = Vec::with_capacity(count.min(64));

        for _ in 0..count {
            materials.push(reader.read_optional_u32()?);
        }

        Ok(Self {
            name_hash,
            materials,
            teach_item: reader.read_optional_u32()?,
            uuid: reader.read_string()?,
        })
    }
}

/// Width of a ranged field whose declared maximum is `max`.
///
/// The same rule the components use: the client's `NumBitsRequired` returns the leading-zero
/// count, so the width is `32 - that`, which is `32 - leading_zeros(max)`.
const fn ranged_bits(max: u32) -> u32 {
    32 - max.leading_zeros()
}

/// The count boundary the encoding switches width at.
///
/// At or below this a count is 7 bits; above it, 17. The client decides which by reading the
/// flag bit in front, so a writer that picks the wrong one desynchronises everything after it.
const SMALL_COUNT: u32 = 64;

/// The wide maximum, once a value is over [`SMALL_COUNT`].
const LARGE_COUNT: u32 = 0x1_0000;

/// What sits in one inventory slot: an item, how many, and its identity.
///
/// The parameter `inventoryslotdata` on `BasicInventoryItem` (sync index 2), and the payload
/// the client reads to draw a stack in the rucksack.
///
/// Layout from `SkySaga.Game/Packets/Common/InventorySlotData.cs`. Three fields are still
/// unnamed there and are kept rather than dropped: they are in the middle of the structure, so
/// leaving them out would shift everything after them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventorySlotData {
    /// `CRC32` of a `geodata.json > Resources > Name`, such as `Dirt`.
    pub name: Option<u32>,

    /// How many are in the stack.
    pub count: u32,

    /// Unnamed in the C#. False in everything observed.
    pub unknown3: bool,
    /// Unnamed in the C#. Zero in everything observed.
    pub unknown4: u32,
    /// Unnamed in the C#. Zero in everything observed.
    pub unknown5: u32,

    /// This stack's own identity, so the client can tell two piles of dirt apart.
    pub item_uuid: String,
}

impl InventorySlotData {
    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_optional_u32(self.name);

        Self::write_counted(writer, self.count);

        writer.write_bit(self.unknown3);

        Self::write_counted(writer, self.unknown4);

        writer.write_bits_le(self.unknown5, ranged_bits(LARGE_COUNT));

        writer.write_string(&self.item_uuid);
    }

    /// A count, and the flag saying how wide it is.
    fn write_counted(writer: &mut BitWriter, value: u32) {
        let large = value > SMALL_COUNT;

        writer.write_bit(large);

        writer.write_bits_le(
            value,
            ranged_bits(if large { LARGE_COUNT } else { SMALL_COUNT }),
        );
    }
}
