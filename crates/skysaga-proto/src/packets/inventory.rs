//! The inventory packets: every drag, drop, split and equip the rucksack UI sends.
//!
//! All client to server, which is what makes them awkward. The client has no serialiser for
//! them -- it only writes them -- so there is nothing in the binary to read a layout off, and
//! each one here was pinned down by dragging an item whose source and target were known and
//! decoding the bytes that came back. The captures are in `tests/inventory.rs`.
//!
//! # The two field orders
//!
//! [`InventoryItemTransferToSlot`] puts **both entity ids first** and then the slots;
//! [`InventoryItemSwap`] **interleaves** them, entity then slot, twice. They are otherwise so
//! similar that this is the easiest mistake to make, and it decodes the second entity id out
//! of the first slot's bits.
//!
//! # Why the client sends both
//!
//! A drop onto an **empty** square is `InventoryItemTransferToSlot`; a drop onto an
//! **occupied** one is `InventoryItemSwap`. That is why stack merging has to live in the swap
//! handler as well as the transfer's -- topping up a stack is a drop onto an occupied square.
//!
//! # Field widths
//!
//! Slots are 6 bits, which is `32 - NumBitsRequired(45)` for the 45-entry inventory list, and
//! counts are 8. Both are under a byte, so the C#'s big-endian `TryReadBitsValue` and this
//! crate's little-endian [`read_bits_le`](crate::bitstream::BitReader::read_bits_le) agree
//! exactly; nothing here is wide enough for the difference to show.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// Enough to address the 45 inventory slots: `32 - NumBitsRequired(45)`.
const SLOT_BITS: u32 = 6;

/// A stack size. Confirmed against splits: half of 50 arrives as 25, a partial drag of 5 as 5.
const COUNT_BITS: u32 = 8;

/// The hotbar's squares.
const HOTBAR_SLOT_BITS: u32 = 5;

/// Drag and drop onto an **empty** square: move `count` of `source_slot` to `target_slot`.
///
/// A `count` smaller than the source stack is a split rather than a move.
///
/// ```text
/// B9            id (ID_USER_PACKET_ENUM + 51)
/// 00 00 00 0A   source entity  32
/// 00 00 00 0A   target entity  32
/// 24 04 A0      source slot     6   001001   =  9
///               count           8   00000001 =  1
///               target slot     6   001010   = 10
///               padding         4
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryItemTransferToSlot {
    pub source_entity: u32,
    pub source_slot: u32,
    pub target_entity: u32,
    pub target_slot: u32,
    /// How much of the stack to move. Zero means the whole thing.
    pub count: u32,
}

impl InventoryItemTransferToSlot {
    pub const ID: u16 = 51;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.source_entity);
        writer.write_u32(self.target_entity);

        writer.write_bits_le(self.source_slot, SLOT_BITS);
        writer.write_bits_le(self.count, COUNT_BITS);
        writer.write_bits_le(self.target_slot, SLOT_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            source_entity: reader.read_u32()?,
            target_entity: reader.read_u32()?,
            source_slot: reader.read_bits_le(SLOT_BITS)?,
            count: reader.read_bits_le(COUNT_BITS)?,
            target_slot: reader.read_bits_le(SLOT_BITS)?,
        })
    }
}

/// Drag and drop onto an **occupied** square: merge the two stacks, or exchange them.
///
/// Note the field order: entity, slot, entity, slot, unlike the transfer's two ids first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryItemSwap {
    pub source_entity: u32,
    pub source_slot: u32,
    pub target_entity: u32,
    pub target_slot: u32,
}

impl InventoryItemSwap {
    pub const ID: u16 = 53;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.source_entity);
        writer.write_bits_le(self.source_slot, SLOT_BITS);
        writer.write_u32(self.target_entity);
        writer.write_bits_le(self.target_slot, SLOT_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            source_entity: reader.read_u32()?,
            source_slot: reader.read_bits_le(SLOT_BITS)?,
            target_entity: reader.read_u32()?,
            target_slot: reader.read_bits_le(SLOT_BITS)?,
        })
    }
}

/// "Take All" in the loot window: every stack from one inventory into another.
///
/// The simplest of the set -- two entity ids, no slots and no counts.
///
/// ```text
/// BA            id (ID_USER_PACKET_ENUM + 52)
/// 00 00 00 0C   source entity (the chest)
/// 00 00 00 0E   target entity (the player)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryItemTransferAll {
    pub source_entity: u32,
    pub target_entity: u32,
}

impl InventoryItemTransferAll {
    pub const ID: u16 = 52;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.source_entity);
        writer.write_u32(self.target_entity);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            source_entity: reader.read_u32()?,
            target_entity: reader.read_u32()?,
        })
    }
}

/// Dropping a stack on the rucksack's trash can.
///
/// ```text
/// B3            id (ID_USER_PACKET_ENUM + 45)
/// 00 00 00 0A   entity  32   the inventory's owner
/// .. ..         slot     6
///               count    8   how much of the stack to destroy
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryItemDestroy {
    pub entity_id: u32,
    pub slot: u32,
    /// How much of the stack to destroy. Zero means all of it.
    pub count: u32,
}

impl InventoryItemDestroy {
    pub const ID: u16 = 45;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.slot, SLOT_BITS);
        writer.write_bits_le(self.count, COUNT_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
            slot: reader.read_bits_le(SLOT_BITS)?,
            count: reader.read_bits_le(COUNT_BITS)?,
        })
    }
}

/// Equipping something from the rucksack.
///
/// Equipment and the rucksack share one 45-entry list, so equipping is a move into one of the
/// low indices rather than a separate store.
///
/// ```text
/// 93            id (ID_USER_PACKET_ENUM + 13)
///               equip slot   4    2 Head, 3 Torso, 4 Legs, 5 Arms
///               entity      32
///               bag slot     6
///               trailing     6    `100000` in every capture; meaning unknown
/// ```
///
/// **Arms is 5 and Legs is 4**, which is the opposite of the obvious guess, and is why the
/// four captures in the tests are one per armour type rather than one example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestEquipInventoryItem {
    /// Destination equipment index. 0 and 1 are the hands, which are not storage.
    pub equip_slot: u32,
    pub entity_id: u32,
    pub bag_slot: u32,
    /// The six bits after the bag slot, kept so the packet round-trips.
    ///
    /// `100000` across all four armour types and two source slots, so nothing yet indicates
    /// what they are. Preserved rather than dropped: a capture that ever differs then shows
    /// up as a changed value instead of vanishing.
    pub trailing: u32,
}

impl RequestEquipInventoryItem {
    pub const ID: u16 = 13;

    /// The width of [`Self::trailing`].
    const TRAILING_BITS: u32 = 6;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.equip_slot, 4);
        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.bag_slot, SLOT_BITS);
        writer.write_bits_le(self.trailing, Self::TRAILING_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            equip_slot: reader.read_bits_le(4)?,
            entity_id: reader.read_u32()?,
            bag_slot: reader.read_bits_le(SLOT_BITS)?,
            // Tolerated as absent: only the four captures prove these bits are always sent.
            trailing: reader.read_bits_le(Self::TRAILING_BITS).unwrap_or(0),
        })
    }
}

/// Binding an item to a hotbar square.
///
/// The hotbar is **not storage**: `hotbarslotresources` holds item name *hashes*, so a bound
/// item legitimately stays in the rucksack. What looks like a duplicate is one stack
/// referenced from two places.
///
/// ```text
///   slot        5     hotbar square
///   resource   32     item name hash (GeoData Resources)
///   unknown     4     zero in every capture
///   itemUUID   str    standard framing
/// ```
///
/// The resource offset of bit 5 is how the layout was found: a 32-bit window slid over the
/// payload until a known hash appeared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestUiSettingsSlotChange {
    pub slot: u32,
    /// `Util.ComputeCrc32` of a GeoData resource name.
    pub resource: u32,
    /// Zero in every capture; purpose unknown, kept so the packet round-trips.
    pub unknown: u32,
    pub item_uuid: String,
}

impl RequestUiSettingsSlotChange {
    pub const ID: u16 = 15;

    const UNKNOWN_BITS: u32 = 4;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.slot, HOTBAR_SLOT_BITS);
        writer.write_u32(self.resource);
        writer.write_bits_le(self.unknown, Self::UNKNOWN_BITS);
        writer.write_string(&self.item_uuid);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            slot: reader.read_bits_le(HOTBAR_SLOT_BITS)?,
            resource: reader.read_u32()?,
            unknown: reader.read_bits_le(Self::UNKNOWN_BITS)?,
            item_uuid: reader.read_string()?,
        })
    }
}

/// Selecting a different hotbar square: the mouse wheel, or the number keys.
///
/// Two bytes on the wire, so the body is this one field. Worth tracking because placing a
/// block and digging arrive as the same [`PerformVoxelActions`](super) packet, and which one
/// it is depends on whether the selected square holds a placeable block or a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestUiSettingsSetActiveSlot {
    pub slot: u32,
}

impl RequestUiSettingsSetActiveSlot {
    pub const ID: u16 = 16;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.slot, HOTBAR_SLOT_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            slot: reader.read_bits_le(HOTBAR_SLOT_BITS)?,
        })
    }
}
