//! The world handshake: what the server sends between "client connected" and "in the world".
//!
//! The full sequence, captured off the wire from the C# server
//! (`cargo run -p raknet --example capture-handshake`):
//!
//! ```text
//! C->S ClientConnected                135
//! S->C ServerInfo                     192   78 bytes
//! S->C MapDefinition                  140    8
//! C->S ClientReadyToSync              136
//! S->C BeginSync                      141    3
//! S->C ChunkSync            x16       142   32778 each
//! C->S ClientInitialSyncFinished      138
//! S->C EntityAdd            x12       234   23..329
//! S->C ClientEntitiesSyncFinished     139    1
//! C->S ClientReadyToPlay              137
//! S->C SetClientEntity                238    5
//! S->C DebugRequestFinishTutorial     162    1
//! ```
//!
//! Wire id = ordinal + `ID_USER_PACKET_ENUM` (134); the `ID` constants here are ordinals.
//!
//! **Both 32-bit conventions appear in this module**, and mixing them up is silent. Ranged
//! fields use [`BitWriter::write_bits_le`] (the emulator's `WriteBits(GetBytes(v), n, true)`
//! idiom); whole-word fields use [`BitWriter::write_u32`], which is big-endian. See the
//! `bitstream` module docs.


use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// Width of a value whose declared maximum is `max`, as the client computes it.
///
/// `32 - NumBitsRequired(max)` in the C#, which is `32 - leading_zeros(max)`.

/// `MapDefinition` — the world's dimensions, biome and game mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapDefinition {
    /// Chunk counts on each axis, max 32 each, so 6 bits apiece.
    pub size_chunks: [u32; 3],

    /// `CRC32` of a `geodata.json > Biomes` name, e.g. `Sky_Island`.
    pub biome: Option<u32>,

    /// 3 bits. The meanings of 1..4 are not established.
    pub game_mode: u32,
}

impl MapDefinition {
    pub const ID: u16 = 6;

    /// Max chunks per axis, which sets the field width.
    const MAX_CHUNKS: u32 = 32;
    const MAX_GAME_MODE: u32 = 4;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        for axis in self.size_chunks {
            writer.write_bits_le(axis, ranged_bits(Self::MAX_CHUNKS));
        }

        // Optional, then a *big-endian* word -- the C# uses bitStream.Write(int) here, not
        // the WriteBits idiom the fields around it use.
        writer.write_optional_u32(self.biome);

        writer.write_bits_le(self.game_mode, ranged_bits(Self::MAX_GAME_MODE));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let mut size_chunks = [0u32; 3];

        for axis in &mut size_chunks {
            *axis = reader.read_bits_le(ranged_bits(Self::MAX_CHUNKS))?;
        }

        Ok(Self {
            size_chunks,
            biome: reader.read_optional_u32()?,
            game_mode: reader.read_bits_le(ranged_bits(Self::MAX_GAME_MODE))?,
        })
    }
}

/// `BeginSync` — how many `ChunkSync` packets are about to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginSync {
    pub chunk_count: u32,
}

impl BeginSync {
    pub const ID: u16 = 7;

    /// The declared maximum, giving a 16-bit field.
    const MAX_CHUNKS: u32 = 0x8000;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_bits_le(self.chunk_count, ranged_bits(Self::MAX_CHUNKS));
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            chunk_count: reader.read_bits_le(ranged_bits(Self::MAX_CHUNKS))?,
        })
    }
}

/// `SetClientEntity` — which entity the client is playing.
///
/// The client takes this as "you are this entity", and addresses it thereafter: the character
/// creator's `SetCharacterCustomisationData` carries the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetClientEntity {
    pub entity_id: u32,
}

impl SetClientEntity {
    pub const ID: u16 = 104;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
        })
    }
}

/// `EntitySync` — this entity has changed.
///
/// The way to update an entity the client already holds. A repeat [`EntityAdd`] must not be
/// used for that: the client destroys the entity and builds a fresh one, and every slot list
/// still naming the old object is left holding a dangling pointer.
///
/// Same framing as `EntityAdd`'s payload, without the name hash or parent: the client already
/// knows what the entity is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySync {
    pub id: u32,
    pub sync_data: Bits,
}

impl EntitySync {
    pub const ID: u16 = 101;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_u32(self.id);
        writer.write_bits_le(self.sync_data.len() as u32, LENGTH_BITS);

        self.sync_data.encode(writer);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let id = reader.read_u32()?;
        let bits = reader.read_bits_le(LENGTH_BITS)? as usize;

        Ok(Self {
            id,
            sync_data: Bits::decode(reader, bits)?,
        })
    }
}

/// `EntityRemoved` — this entity is gone.
///
/// Sent when a player disconnects, so the others stop drawing their body. Without it the
/// departed player stands there until the client is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityRemoved {
    pub entity_id: u32,
}

impl EntityRemoved {
    pub const ID: u16 = 103;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_u32(self.entity_id);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            entity_id: reader.read_u32()?,
        })
    }
}

/// `ClientEntitiesSyncFinished` — no body; the id is the whole message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientEntitiesSyncFinished;

impl ClientEntitiesSyncFinished {
    pub const ID: u16 = 5;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }
}

/// `DebugRequestFinishTutorial` — no body.
///
/// Without it the client stays in tutorial mode and spills hint text into the chat log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugRequestFinishTutorial;

impl DebugRequestFinishTutorial {
    pub const ID: u16 = 28;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }
}

/// `ServerInfo` — who owns this world, what it is, and where chat lives.
///
/// The first thing the server sends after `ClientConnected`, and the client acts on several
/// of its fields well beyond the loading screen:
///
/// - `owner_guid` is compared against the player entity's owner component to decide whether
///   the world is *yours*. A mismatch makes home-island-locked items ("only on your home
///   island") refuse to place.
/// - `is_home_world` / `is_my_world` gate the same placement rules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerInfo {
    /// The player's own character uuid, as a string. Must match the owner component.
    pub owner_guid: String,
    pub owner_name: String,
    /// A `geodata.json > Biomes` name.
    pub biome: String,
    /// `CRC32` of the adventure name, when the world is one.
    pub adventure: Option<u32>,
    pub map_header_seed: u32,
    pub is_home_world: bool,
    pub is_my_world: bool,
    pub chat_host: String,
    pub chat_port: u16,
}

impl ServerInfo {
    pub const ID: u16 = 58;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_string(&self.owner_guid);
        writer.write_string(&self.owner_name);
        writer.write_string(&self.biome);

        writer.write_optional_u32(self.adventure);
        writer.write_u32(self.map_header_seed);

        writer.write_bit(self.is_home_world);
        writer.write_bit(self.is_my_world);

        writer.write_string(&self.chat_host);
        writer.write_u16(self.chat_port);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            owner_guid: reader.read_string()?,
            owner_name: reader.read_string()?,
            biome: reader.read_string()?,
            adventure: reader.read_optional_u32()?,
            map_header_seed: reader.read_u32()?,
            is_home_world: reader.read_bit()?,
            is_my_world: reader.read_bit()?,
            chat_host: reader.read_string()?,
            chat_port: reader.read_u16()?,
        })
    }
}

/// The 18-bit length field used for both `EntityAdd`'s sync data and the parameter payload
/// inside it: `32 - NumBitsRequired(0x20000)`.
const LENGTH_BITS: u32 = ranged_bits(0x2_0000);

/// An opaque run of bits, carried and re-emitted unchanged.
///
/// Used where this crate frames a payload it does not (yet) interpret — an entity's parameter
/// data, a chunk's voxels. Keeping it opaque is what lets the framing be tested byte-exactly
/// against a capture before a single component exists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Bits {
    bytes: Vec<u8>,
    len: usize,
}

impl Bits {
    pub fn new(bytes: Vec<u8>, len: usize) -> Self {
        Self { bytes, len }
    }

    /// Take the contents of a writer as an opaque payload.
    pub fn from_writer(writer: &BitWriter) -> Self {
        Self {
            bytes: writer.as_bytes().to_vec(),
            len: writer.bits_used(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn encode(&self, writer: &mut BitWriter) {
        writer.write_bits_msb(&self.bytes, self.len);
    }

    fn decode(reader: &mut BitReader, len: usize) -> Result<Self, BitError> {
        Ok(Self {
            bytes: reader.read_bits_msb(len)?,
            len,
        })
    }
}

/// `EntityAdd` — "this entity now exists, and here is its state".
///
/// One per world entity during the initial sync, and again whenever something spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityAdd {
    /// `CRC32` of an `Entities.json > Entities > Name`.
    pub name_hash: Option<u32>,
    pub id: u32,
    pub parent_id: Option<u32>,

    /// The entity's synced parameters. See [`SyncData`] for its internal shape; `EntityAdd`
    /// itself only frames it with an 18-bit length.
    pub sync_data: Bits,
}

impl EntityAdd {
    pub const ID: u16 = 100;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_optional_u32(self.name_hash);
        writer.write_u32(self.id);
        writer.write_optional_u32(self.parent_id);

        writer.write_bits_le(self.sync_data.len() as u32, LENGTH_BITS);

        self.sync_data.encode(writer);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let name_hash = reader.read_optional_u32()?;
        let id = reader.read_u32()?;
        let parent_id = reader.read_optional_u32()?;

        let len = reader.read_bits_le(LENGTH_BITS)? as usize;

        Ok(Self {
            name_hash,
            id,
            parent_id,
            sync_data: Bits::decode(reader, len)?,
        })
    }
}

/// The body of an entity's sync: which parameters are present, then their values.
///
/// ```text
/// flags       one bit per synced parameter (the entity's declared count)
/// length      18 bits -- how many bits of parameter data follow
/// parameters  the values, in ascending parameter index
/// ```
///
/// The flag block is written as whole 32-bit words in **big-endian order**. The C# builds it
/// with `BitArray.CopyTo`, which lays the words out little-endian, and then reverses each
/// four-byte group to undo that — so the wire order is most-significant-word-first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncData {
    /// One entry per synced parameter the entity declares — `Player` has 89.
    pub present: Vec<bool>,
    pub parameters: Bits,
}

impl SyncData {
    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_bits(&Self::flag_bytes(&self.present), self.present.len() as u32);

        writer.write_bits_le(self.parameters.len() as u32, LENGTH_BITS);

        self.parameters.encode(writer);
    }

    /// Pack the flags the way the C# does, which is **not** "index 0 first".
    ///
    /// `BitArray.CopyTo` writes bit `i` into byte `i / 8` at position `i % 8` -- least
    /// significant first -- and then every *complete* four-byte group is reversed. `WriteBits`
    /// then emits each byte most-significant-bit first. The net effect is that within a byte
    /// the flags come out backwards, and within a 32-bit word the bytes do too.
    ///
    /// Note the reversal loop runs `count / 32` times, so a trailing partial word is not
    /// reversed -- an entity with 15 synced parameters gets no reversal at all.
    fn flag_bytes(present: &[bool]) -> Vec<u8> {
        let mut data = vec![0u8; present.len().div_ceil(8)];

        for (index, &flag) in present.iter().enumerate() {
            if flag {
                data[index / 8] |= 1 << (index % 8);
            }
        }

        for group in 0..(present.len() / 32) {
            data[group * 4..group * 4 + 4].reverse();
        }

        data
    }

    /// The inverse of [`Self::flag_bytes`].
    fn flags_from_bytes(mut data: Vec<u8>, count: usize) -> Vec<bool> {
        for group in 0..(count / 32) {
            data[group * 4..group * 4 + 4].reverse();
        }

        (0..count)
            .map(|index| data[index / 8] & (1 << (index % 8)) != 0)
            .collect()
    }

    /// `count` is the entity's declared synced-parameter count, which the reader cannot infer
    /// from the stream — it comes from `Entities.json`.
    pub fn decode(reader: &mut BitReader, count: usize) -> Result<Self, BitError> {
        // Read the flag block back as bytes in RakNet's own layout: whole bytes
        // most-significant-first, a trailing partial byte into its *low* bits.
        let mut data = vec![0u8; count.div_ceil(8)];
        let mut remaining = count;
        let mut index = 0;

        while remaining > 0 {
            let width = remaining.min(8);
            let mut byte = 0u8;

            for bit in 0..width {
                if reader.read_bit()? {
                    byte |= 0x80 >> bit;
                }
            }

            data[index] = if width == 8 { byte } else { byte >> (8 - width) };

            remaining -= width;
            index += 1;
        }

        let present = Self::flags_from_bytes(data, count);

        let len = reader.read_bits_le(LENGTH_BITS)? as usize;

        Ok(Self {
            present,
            parameters: Bits::decode(reader, len)?,
        })
    }

    /// Indices of the parameters that carry a value.
    pub fn present_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.present
            .iter()
            .enumerate()
            .filter_map(|(index, &flag)| flag.then_some(index))
    }
}

/// `ChunkSync` — one chunk of terrain.
///
/// The bulk of the handshake: 16 of these at ~32 KB each on the home island. The two data
/// arrays are written with `WriteAlignedBytes`, so each is preceded by zero padding up to a
/// byte boundary — which is why the packet is a whole number of bytes rather than the tight
/// bit packing everything else uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkSync {
    /// Chunk coordinates, 6 bits each (max 32 per axis, as in [`MapDefinition`]).
    pub coords: [u32; 3],

    /// Voxel data. Contents are not interpreted here.
    pub data1: Option<Vec<u8>>,
    pub data2: Option<Vec<u8>>,

    /// Which neighbouring chunks exist, as a 7-bit mask
    /// (`8 - NumBitsRequiredByte(64)`).
    pub adjacent_chunks: Option<u8>,
}

impl ChunkSync {
    pub const ID: u16 = 8;

    const MAX_CHUNKS: u32 = 32;
    /// `8 - num_bits_required_byte(64)` — the neighbour mask's width.
    const ADJACENT_BITS: u32 = 8 - 64u8.leading_zeros();

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        for axis in self.coords {
            writer.write_bits_le(axis, ranged_bits(Self::MAX_CHUNKS));
        }

        for data in [&self.data1, &self.data2] {
            writer.write_bit(data.is_some());

            if let Some(data) = data {
                writer.write_u32(data.len() as u32);
                writer.write_aligned_bytes(data);
            }
        }

        writer.write_bit(self.adjacent_chunks.is_some());

        if let Some(mask) = self.adjacent_chunks {
            writer.write_bits_le(u32::from(mask), Self::ADJACENT_BITS);
        }
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let mut coords = [0u32; 3];

        for axis in &mut coords {
            *axis = reader.read_bits_le(ranged_bits(Self::MAX_CHUNKS))?;
        }

        let mut arrays = [None, None];

        for slot in &mut arrays {
            if reader.read_bit()? {
                let len = reader.read_u32()? as usize;

                *slot = Some(reader.read_aligned_bytes(len)?);
            }
        }

        let [data1, data2] = arrays;

        let adjacent_chunks = if reader.read_bit()? {
            Some(reader.read_bits_le(Self::ADJACENT_BITS)? as u8)
        } else {
            None
        };

        Ok(Self {
            coords,
            data1,
            data2,
            adjacent_chunks,
        })
    }
}
