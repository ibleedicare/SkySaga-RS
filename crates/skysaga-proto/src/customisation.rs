//! A character's appearance: `CustomisationData`.
//!
//! This is the payload of `SetCharacterCustomisationData` (packet 37) and of the `Player`
//! entity's `customisationdata` parameter, sync index 19. Reversed in
//! `documentations/character-and-appearance.md` §5; the encoding below is checked against
//! captures from the real RakNet BitStream in `tests/character_creation.rs`.
//!
//! ```text
//! gender       2 bits
//! tribe        optional u32
//! materials    count-optimised list, default 3, of optional u32
//! attachments  count-optimised list, default 1, of (optional u32, optional u32)
//! ```
//!
//! Every id is `CRC32(name.to_lowercase())` over a *GeoData* name — see
//! [`skysaga_core::name_hash`]. The names come from `geodata.json`: `Tribes` (Cat, Human,
//! Lizard), `Materials` (by `Category`: `CharacterSkin`, `CharacterEyes`,
//! `CharacterDefaultClothing`, `CharacterHair`) and `CharacterAttachments` (18 hairstyles).
//!
//! Appearance does **not** live on `PlayerAspectsComponent` — that component is permissions.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// Width of the `gender` field. `NumBitsRequired(2)`, so three values are representable even
/// though the client's name table only holds `Male` and `Female`.
const GENDER_BITS: u32 = 2;

/// `materials` always holds skin, eyes and clothing.
pub const DEFAULT_MATERIAL_COUNT: usize = 3;

/// `attachments` always holds exactly the hair.
pub const DEFAULT_ATTACHMENT_COUNT: usize = 1;

const SKIN: usize = 0;
const EYES: usize = 1;
const CLOTHING: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Male,
    Female,

    /// The field is two bits wide, so the client *can* send a third value. It is not known
    /// whether index 2 means "unset" or is simply unused, so it is preserved rather than
    /// rejected — a decode failure here would drop the whole packet.
    Unknown(u32),
}

impl Gender {
    pub fn value(self) -> u32 {
        match self {
            Self::Male => 0,
            Self::Female => 1,
            Self::Unknown(value) => value,
        }
    }

    pub fn encode(self, writer: &mut BitWriter) {
        writer.write_bits_le(self.value(), GENDER_BITS);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(match reader.read_bits_le(GENDER_BITS)? {
            0 => Self::Male,
            1 => Self::Female,
            other => Self::Unknown(other),
        })
    }
}

impl Default for Gender {
    fn default() -> Self {
        Self::Male
    }
}

/// One attachment slot: what is attached, and what it is coloured with.
///
/// Slot 0 is the hairstyle and its colour. The schema allows more, but nothing in the client
/// was found that writes a second one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attachment {
    /// `CRC32(CharacterAttachment.Name)`.
    pub attachment: Option<u32>,
    /// `CRC32(Material.Name)`.
    pub material: Option<u32>,
}

impl Attachment {
    fn encode(&self, writer: &mut BitWriter) {
        writer.write_optional_u32(self.attachment);
        writer.write_optional_u32(self.material);
    }

    fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            attachment: reader.read_optional_u32()?,
            material: reader.read_optional_u32()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomisationData {
    pub gender: Gender,

    /// `CRC32(Tribe.Name)` — Cat, Human or Lizard.
    pub tribe: Option<u32>,

    /// Positional: `[0]` skin, `[1]` eyes, `[2]` clothing. Use [`Self::skin`] and friends
    /// rather than indexing.
    pub materials: Vec<Option<u32>>,

    /// Positional: `[0]` is the hair. See [`Self::hair_style`] / [`Self::hair_colour`].
    pub attachments: Vec<Attachment>,
}

impl Default for CustomisationData {
    /// The cheapest wire-legal value: the schema's default list lengths, everything unset.
    ///
    /// The lengths matter — a `CustomisationData` with the wrong number of slots encodes with
    /// the escape path and costs 33 extra bits per list.
    fn default() -> Self {
        Self {
            gender: Gender::default(),
            tribe: None,
            materials: vec![None; DEFAULT_MATERIAL_COUNT],
            attachments: vec![Attachment::default(); DEFAULT_ATTACHMENT_COUNT],
        }
    }
}

impl CustomisationData {
    /// `CRC32` of the `CharacterSkin` material.
    pub fn skin(&self) -> Option<u32> {
        self.material(SKIN)
    }

    /// `CRC32` of the `CharacterEyes` material.
    pub fn eyes(&self) -> Option<u32> {
        self.material(EYES)
    }

    /// `CRC32` of the `CharacterDefaultClothing` material.
    pub fn clothing(&self) -> Option<u32> {
        self.material(CLOTHING)
    }

    /// `CRC32` of the `CharacterAttachment` in slot 0.
    pub fn hair_style(&self) -> Option<u32> {
        self.attachments.first().and_then(|hair| hair.attachment)
    }

    /// `CRC32` of the `CharacterHair` material in slot 0.
    pub fn hair_colour(&self) -> Option<u32> {
        self.attachments.first().and_then(|hair| hair.material)
    }

    fn material(&self, slot: usize) -> Option<u32> {
        self.materials.get(slot).copied().flatten()
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        self.gender.encode(writer);
        writer.write_optional_u32(self.tribe);

        write_count(writer, self.materials.len(), DEFAULT_MATERIAL_COUNT);

        for material in &self.materials {
            writer.write_optional_u32(*material);
        }

        write_count(writer, self.attachments.len(), DEFAULT_ATTACHMENT_COUNT);

        for attachment in &self.attachments {
            attachment.encode(writer);
        }
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let gender = Gender::decode(reader)?;
        let tribe = reader.read_optional_u32()?;

        let material_count = read_count(reader, DEFAULT_MATERIAL_COUNT)?;
        let mut materials = Vec::with_capacity(material_count.min(MAX_LIST_RESERVE));

        for _ in 0..material_count {
            materials.push(reader.read_optional_u32()?);
        }

        let attachment_count = read_count(reader, DEFAULT_ATTACHMENT_COUNT)?;
        let mut attachments = Vec::with_capacity(attachment_count.min(MAX_LIST_RESERVE));

        for _ in 0..attachment_count {
            attachments.push(Attachment::decode(reader)?);
        }

        Ok(Self {
            gender,
            tribe,
            materials,
            attachments,
        })
    }
}

/// A peer-supplied count can be up to `u32::MAX`; never reserve on it directly.
const MAX_LIST_RESERVE: usize = 64;

/// The count encoding, which is unusual enough to be worth stating.
///
/// The count field itself is **zero bits wide** — `NumBitsRequired(0)` is 0, verified on both
/// sides in the client (`FUN_0084df30` writes it, `FUN_008827e0` reads 0 bits and returns
/// `0 + 3`). So the count is implicit, and what is actually on the wire is:
///
/// ```text
/// escape  1 bit    0 = the count is exactly the default
///                  1 = a full 32-bit count follows
/// count   32 bits  only when escape == 1
/// ```
///
/// This is *not* the count-optimised list used elsewhere in the protocol; that one has a real
/// width. Reusing the generic helper here would put extra bits on the wire.
fn write_count(writer: &mut BitWriter, count: usize, default: usize) {
    if count == default {
        writer.write_bit(false);
    } else {
        writer.write_bit(true);
        writer.write_u32(count as u32);
    }
}

fn read_count(reader: &mut BitReader, default: usize) -> Result<usize, BitError> {
    if reader.read_bit()? {
        Ok(reader.read_u32()? as usize)
    } else {
        Ok(default)
    }
}
