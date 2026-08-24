//! Crafting: queue a recipe, collect what it made.
//!
//! Two UIs share these packets. A **station** craft names the Anvil or Workbench entity; a
//! **hand** craft names the player's own entity, because the player carries a crafting
//! component of its own. Nothing in the packets distinguishes them, which is why the server
//! validates the station against the recipe rather than against a flag.
//!
//! # A recipe is addressed by its own name
//!
//! [`QueueRecipeOnEntity::item_id`] is the name hash of the recipe's **own name**, despite
//! being spelled `itemID`. A live client queuing `Hand_Craft_Carved_Stone_Piece` sends
//! `2767626641`, which is that name's hash; its output, `Carved_Stone_Piece`, hashes elsewhere.
//!
//! `crafting.md` says the opposite — that the field is the hash of the output resource — and
//! that is wrong. It matters because it is the *same* id the recipe book's `recipelist`
//! carries, so the book and the queue speak one scheme rather than two, and a server that
//! keyed its lookup on outputs refuses every craft the client asks for.
//!
//! # Silence hangs the panel
//!
//! `FUN_00795f00`, the [`CraftingFailed`] handler, is the only thing that takes the client out
//! of its "Crafting phase" state. A queue the server refuses without answering leaves the
//! window spinning until the player reopens it, so every rejection has to send one.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// The default length of `selectedItemSpecs`, which sets its count width.
const SPEC_LIST_DEFAULT: u32 = 5;

/// The default length of an [`ItemSpec`]'s `subResources`.
const SUB_RESOURCE_DEFAULT: u32 = 4;

/// The client clamps a crafting slot id to twelve before sending it.
const MAX_CRAFTING_SLOT: u32 = 12;

/// A count-optimised list length.
///
/// `min(count, max)` in `NumBitsRequired(max)` bits; at the maximum a further bit says whether
/// the real length is exactly that (`0`) or follows as a full 32 bits (`1`). Reading it any
/// other way desynchronises everything after the list rather than merely getting the count
/// wrong, which is why this is shared rather than written out per packet.
fn read_count(reader: &mut BitReader, max: u32) -> Result<u32, BitError> {
    let count = reader.read_bits_le(ranged_bits(max))?;

    if count < max {
        return Ok(count);
    }

    if reader.read_bit()? {
        reader.read_u32()
    } else {
        Ok(max)
    }
}

fn write_count(writer: &mut BitWriter, count: u32, max: u32) {
    writer.write_bits_le(count.min(max), ranged_bits(max));

    if count < max {
        return;
    }

    if count == max {
        writer.write_bit(false);
    } else {
        writer.write_bit(true);
        writer.write_u32(count);
    }
}

/// One ingredient the player chose, as the crafting UI reports it.
///
/// The server does not need any of it to carry out a craft: the recipe's own `Input` list
/// already says what to consume. It matters for material variants, which pick *which* metal or
/// wood a sword is made of. Decoded in full anyway, because the fields sit in front of nothing
/// -- a partial read leaves the stream misaligned for the next packet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemSpec {
    pub resource: Option<u32>,
    pub sub_resources: Vec<Option<u32>>,
    pub material_resource: Option<u32>,
    pub item_uuid: String,
}

impl ItemSpec {
    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let resource = reader.read_optional_u32()?;

        let count = read_count(reader, SUB_RESOURCE_DEFAULT)?;

        let mut sub_resources = Vec::with_capacity(count.min(64) as usize);

        for _ in 0..count {
            sub_resources.push(reader.read_optional_u32()?);
        }

        Ok(Self {
            resource,
            sub_resources,
            material_resource: reader.read_optional_u32()?,
            item_uuid: reader.read_string()?,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_optional_u32(self.resource);

        write_count(
            writer,
            self.sub_resources.len() as u32,
            SUB_RESOURCE_DEFAULT,
        );

        for sub in &self.sub_resources {
            writer.write_optional_u32(*sub);
        }

        writer.write_optional_u32(self.material_resource);
        writer.write_string(&self.item_uuid);
    }
}

/// `QueueRecipeOnEntity` (39) — craft this, here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueRecipeOnEntity {
    /// The crafting station. The player's own entity for hand crafting.
    pub entity_id: u32,

    /// Name hash of the **recipe's own name**, despite the field being spelled `itemID`.
    ///
    /// Not the output resource: see the module docs. This is the same id `recipelist` carries.
    pub item_id: Option<u32>,

    pub selected_item_specs: Vec<ItemSpec>,
}

impl QueueRecipeOnEntity {
    pub const ID: u16 = 39;

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let entity_id = reader.read_u32()?;
        let item_id = reader.read_optional_u32()?;

        let count = read_count(reader, SPEC_LIST_DEFAULT)?;

        // A hostile or confused client could claim a huge list. Each spec reads at least a
        // few bits, so a long one fails on the stream's end rather than allocating first.
        let mut selected_item_specs = Vec::with_capacity(count.min(32) as usize);

        for _ in 0..count {
            selected_item_specs.push(ItemSpec::decode(reader)?);
        }

        Ok(Self {
            entity_id,
            item_id,
            selected_item_specs,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
        writer.write_optional_u32(self.item_id);

        write_count(
            writer,
            self.selected_item_specs.len() as u32,
            SPEC_LIST_DEFAULT,
        );

        for spec in &self.selected_item_specs {
            spec.encode(writer);
        }
    }
}

/// `CollectCraftedItemInSlot` (40) — take what a finished slot holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CollectCraftedItemInSlot {
    pub entity_id: u32,
    /// Four bits: the client clamps to twelve before sending.
    pub slot: u32,
    /// "Finish it now", the rush-the-timer button.
    pub immediate: bool,
}

impl CollectCraftedItemInSlot {
    pub const ID: u16 = 40;

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
            slot: reader.read_bits_le(ranged_bits(MAX_CRAFTING_SLOT))?,
            immediate: reader.read_bit()?,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
        writer.write_bits_le(self.slot, ranged_bits(MAX_CRAFTING_SLOT));
        writer.write_bit(self.immediate);
    }
}

/// `CraftingQueryQueue` (41) — "what is in this station's queue?"
///
/// There is no dedicated reply. The answer is an `EntitySync` of the station's own
/// `craftingslots`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CraftingQueryQueue {
    pub entity_id: u32,
}

impl CraftingQueryQueue {
    pub const ID: u16 = 41;

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
    }
}

/// `CraftingFailed` (42) — the refusal that unsticks the panel.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CraftingFailed {
    pub entity_id: u32,
    /// What could not be made. The client's handler reads only this field.
    pub resource: Option<u32>,
}

impl CraftingFailed {
    pub const ID: u16 = 42;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
        writer.write_optional_u32(self.resource);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
            resource: reader.read_optional_u32()?,
        })
    }
}
