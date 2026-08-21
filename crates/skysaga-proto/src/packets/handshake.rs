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

use skysaga_core::bits::num_bits_required;

use crate::bitstream::{BitError, BitReader, BitWriter};
use crate::client_build::ClientBuild;

/// Width of a value whose declared maximum is `max`, as the client computes it.
///
/// `32 - NumBitsRequired(max)` in the C#, which is `32 - leading_zeros(max)`.
const fn ranged_bits(max: u32) -> u32 {
    32 - num_bits_required(max)
}

/// `MapSpec` — build 36731's nested world specification. **36731 only**; 10414 has no such
/// struct.
///
/// Layout from the client's `FUN_007e9810`, whose first member is the "searchable" sub-struct
/// `FUN_007e9970`. See `documentations/packets-b36731.md` §8.
///
/// # The fields are numbered, not named
///
/// Most are **GeoData table indices**. §8 recovered the field *names* from the dumper
/// (`biome`, `region`, `adventure`, `difficulty`, `seed`, `timeOfDayPreset`, `mapSize`, …) and
/// the *order* from the deserializer, and states that the two have **not been correlated**. So
/// the slots here carry their wire position and their width, which are known, and not a name,
/// which is not. Naming them on a guess would bury the guess where nobody would question it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapSpec {
    /// The 14 ranged fields of the searchable sub-struct, in wire order.
    ///
    /// Widths come from [`MapSpec::SEARCHABLE_COUNTS`] and are **data-dependent**: the client
    /// computes each from the size of the table it indexes.
    pub searchable: [u32; 14],

    /// Slot 5 of the searchable sub-struct.
    pub searchable_string_a: String,

    /// Slot 6: a plain big-endian word, not a ranged field.
    pub searchable_u32: u32,

    /// Slot 16.
    pub searchable_string_b: String,

    /// The string after the searchable sub-struct.
    pub name: String,

    /// 5 bits, indexing a 28-entry table (`FUN_007d7970`).
    pub time_of_day: u32,

    /// `list<uint32>` (`FUN_007d7af0`): a 4-bit count, then that many big-endian words.
    pub map_list: Vec<u32>,

    /// The string that closes `MapSpec`.
    pub trailing_name: String,
}

impl MapSpec {
    /// GeoData table sizes, in wire order, as measured on a live Alpha V10 client.
    ///
    /// Read from `[0x01487bcc] + offset` with `tools/read_counts_attach.py`. The client derives
    /// each field's width from the table's size, so **these are data, not constants** — a client
    /// with different GeoData produces a different wire format for the same packet, and this
    /// array is what has to be re-measured when that happens.
    ///
    /// Entry 3 (`4`) is not a table at all: that slot is an inline ranged field on a maximum of
    /// 3, which happens to take the same `count - 1` path.
    pub const SEARCHABLE_COUNTS: [u32; 14] = [
        6, 25, 145, 4, // slots 1-4
        46, 46, 46, 46, 17, 6, 6, 32, 11, // slots 7-15
        4,  // slot 17
    ];

    /// Width of a field indexing a `count`-entry table: `32 - clz32(count - 1)`.
    ///
    /// An *index* covers `0..count - 1`, hence the subtraction. Inline ranged fields declare
    /// their maximum directly and use [`ranged_bits`] instead — confusing the two costs a bit,
    /// and one bit shifts everything after it.
    const fn index_bits(count: u32) -> u32 {
        ranged_bits(count - 1)
    }

    fn encode(&self, writer: &mut BitWriter) {
        let width = |slot: usize| Self::index_bits(Self::SEARCHABLE_COUNTS[slot]);

        // --- the searchable sub-struct, FUN_007e9970 -------------------------------------
        for slot in 0..4 {
            writer.write_bits_le(self.searchable[slot], width(slot));
        }

        writer.write_string(&self.searchable_string_a);
        writer.write_u32(self.searchable_u32);

        for slot in 4..13 {
            writer.write_bits_le(self.searchable[slot], width(slot));
        }

        writer.write_string(&self.searchable_string_b);
        writer.write_bits_le(self.searchable[13], width(13));

        // --- the rest of MapSpec, FUN_007e9810 -------------------------------------------
        writer.write_string(&self.name);
        writer.write_bits_le(self.time_of_day, Self::index_bits(28));

        // The list header FUN_007e66b0 opens with FUN_007d77d0, a 4-bit tag ranged on 0xC.
        // Zero means "no elements", and the element loop then never runs.
        writer.write_bits_le(self.map_list.len() as u32, ranged_bits(Self::MAP_LIST_MAX));

        for element in &self.map_list {
            writer.write_u32(*element);
        }

        writer.write_string(&self.trailing_name);
    }

    /// The declared maximum of the map list's count tag, giving a 4-bit field.
    const MAP_LIST_MAX: u32 = 0xC;

    /// The home island, with every GeoData slot filled from build 36731's own `GeoData.json`.
    ///
    /// # Why an index of zero does not work
    ///
    /// Each of the client's tables holds **one more entry than the JSON**: it prepends a "none"
    /// sentinel, which is how the 145/144, 46/45, 25/24 … pairs line up. So a wire index is
    /// `position + 1`, and **index 0 means "nothing"** — a map of all zeros is not an empty map,
    /// it is a map that names nothing, which is exactly why the client parses it and then cannot
    /// resolve a world from it.
    ///
    /// # Where these come from
    ///
    /// Not chosen by hand: `Adventures[82]` (`Home_Island_Adventure`) names its own dependencies,
    /// and the rest follow from it. Slots the adventure leaves empty stay 0, which is the correct
    /// "none" rather than a guess.
    ///
    /// | slot | value | source |
    /// |---|---|---|
    /// | `biome` | 1 | `Biomes[0]` = `Desert` |
    /// | `adventure` | 83 | `Adventures[82]` = `Home_Island_Adventure` |
    /// | `timeOfDayPreset` | 6 | the adventure's `TimeOfDay.TimeOfDayPreset` = `Home_Island` = `TimeOfDayPresets[5]` |
    /// | `mapSize` | 4 | the adventure's `MapSize` = `HomeIsland_6x4x6` = `MapSizes[3]` |
    /// | `mapSizeCategory` | 5 | that map size's `Category` = `Special` = `MapSizeCategories[4]` |
    /// | `activeEvent` | 3 | `Events[2]` = `NoEvent` |
    /// | `featureName` | `Home_Island_World` | the adventure's `RootFeature` |
    ///
    /// The slot-to-name correlation behind this is in `documentations/packets-b36731.md` §8.
    pub fn home_island_b36731(seed: u32) -> Self {
        Self {
            searchable: [
                1,  //  1 biome              Biomes[0] Desert
                0,  //  2 region             the adventure names none
                83, //  3 adventure          Adventures[82] Home_Island_Adventure
                0,  //  4 difficulty         inline, max 3
                0,  //  7 palette            the adventure's BiomePalette is empty
                0,  //  8 featureCreatureSet
                0,  //  9 terrainCreatureSet
                0,  // 10 caveCreatureSet
                6,  // 11 timeOfDayPreset    TimeOfDayPresets[5] Home_Island
                0,  // 12 timeOfDayPresetList  the adventure's is empty
                5,  // 13 mapSizeCategory    MapSizeCategories[4] Special
                4,  // 14 mapSize            MapSizes[3] HomeIsland_6x4x6
                0,  // 15 terrainGenerator   DefaultTerrainType "Sky" is not one of these
                3,  // 17 activeEvent        Events[2] NoEvent
            ],
            // Slot 5, `adventureType`: this adventure entry carries no `AdventureType` key.
            searchable_string_a: String::new(),
            // Slot 6, `seed`: a plain word, and the only field here that is not a table index.
            searchable_u32: seed,
            // Slot 16, `featureName`.
            searchable_string_b: "Home_Island_World".to_owned(),
            name: String::new(),
            // The tail 5-bit field, `cost`: AdventureCosts has no home-island entry.
            time_of_day: 0,
            map_list: Vec::new(),
            trailing_name: String::new(),
        }
    }
}

/// `MapDefinition` — the world's dimensions, biome and game mode.
///
/// # Two builds, two structs
///
/// 36731 shares nothing with 10414 here beyond the name: it carries a nested [`MapSpec`], two
/// strings, an optional uuid and an optional crc. [`Self::encode`] writes one layout or the
/// other according to the writer's build. See `documentations/packets-b36731.md` §8.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapDefinition {
    /// Chunk counts on each axis, max 32 each, so 6 bits apiece. **10414 only.**
    pub size_chunks: [u32; 3],

    /// `CRC32` of a `geodata.json > Biomes` name, e.g. `Sky_Island`. **10414 only.**
    pub biome: Option<u32>,

    /// 3 bits. The meanings of 1..4 are not established. **10414 only.**
    pub game_mode: u32,

    // --- 36731 only ---------------------------------------------------------------------
    /// **36731 only.**
    pub spec: MapSpec,

    /// **36731 only.**
    pub adventure_type: String,

    /// **36731 only.**
    pub map_file_name: String,

    /// **36731 only.** Presence bit, then 16 bytes in .NET `Guid.ToByteArray()` order.
    pub group_lock: Option<[u8; 16]>,

    /// **36731 only.** Presence bit, then a big-endian word.
    pub game_queue_crc: Option<u32>,

    /// **36731 only.** Five bits — see [`MapDefinition::GAME_MODE_MAX_B36731`].
    pub game_mode_b36731: u32,

    /// **36731 only.** A plain word the dumper gives no name for; lands at `+0x10`.
    pub unnamed: u32,
}

impl MapDefinition {
    pub const ID: u16 = 6;

    /// Max chunks per axis, which sets the field width.
    const MAX_CHUNKS: u32 = 32;
    const MAX_GAME_MODE: u32 = 4;

    /// `GameMode`'s declared maximum, giving a **five**-bit field.
    ///
    /// The client's call is `FUN_00ea7260(0x10)` -> 27, so it reads `0x20 - 27` = 5 bits. Note
    /// this is an inline ranged field on a maximum, **not** a table index, so there is no
    /// `count - 1`. The C# reference writes it as an index and emits 4 bits; one bit short here
    /// leaves the trailing word shifted, and everything after it is silently wrong.
    const GAME_MODE_MAX_B36731: u32 = 0x10;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        if writer.build() == ClientBuild::B36731 {
            self.encode_b36731(writer);

            return;
        }

        for axis in self.size_chunks {
            writer.write_bits_le(axis, ranged_bits(Self::MAX_CHUNKS));
        }

        // Optional, then a *big-endian* word -- the C# uses bitStream.Write(int) here, not
        // the WriteBits idiom the fields around it use.
        writer.write_optional_u32(self.biome);

        writer.write_bits_le(self.game_mode, ranged_bits(Self::MAX_GAME_MODE));
    }

    /// The 2017 struct. Field order from the client's deserializer `FUN_007e9e80`.
    ///
    /// The packet id is already written by [`Self::encode`].
    fn encode_b36731(&self, writer: &mut BitWriter) {
        self.spec.encode(writer);

        writer.write_string(&self.adventure_type);
        writer.write_string(&self.map_file_name);

        writer.write_optional_bytes(self.group_lock.as_ref().map(|uuid| &uuid[..]));
        writer.write_optional_u32(self.game_queue_crc);

        writer.write_bits_le(
            self.game_mode_b36731,
            ranged_bits(Self::GAME_MODE_MAX_B36731),
        );
        writer.write_u32(self.unnamed);
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
            // 10414's struct has no 2017 fields to read.
            ..Self::default()
        })
    }
}

/// `SetConnectionTimeout` — how long the client waits before giving up on the connection.
///
/// **Build 36731 only.** It has no 10414 counterpart, so it writes its own id rather than going
/// through the build translation table. Id 11, between `MapDefinition` and `BeginSync`.
///
/// One ranged field, maximum 30000, fifteen bits — read from the client's `FUN_007d75a0`, which
/// asks `clz32(0x7530)` (= 17), reads `0x20 - 17` bits, and clamps the result into `[0, 30000]`.
/// See `tests/set_connection_timeout_b36731.rs` for the disassembly this came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetConnectionTimeout {
    /// Milliseconds. Values above [`Self::MAX_MILLIS`] are clamped, as the client clamps them.
    pub millis: u32,
}

impl SetConnectionTimeout {
    pub const ID_B36731: u16 = 11;

    /// The client's own ceiling, and the maximum the field's width is derived from.
    pub const MAX_MILLIS: u32 = 30_000;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_native_packet_id(Self::ID_B36731);
        writer.write_bits_le(
            self.millis.min(Self::MAX_MILLIS),
            ranged_bits(Self::MAX_MILLIS),
        );
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
///
/// # Two builds, two structs
///
/// Build 36731 (Alpha V10, 2017) does not merely renumber this packet — it is a **different
/// struct**, and only `owner_name`, `chat_host` and `chat_port` survive into it. The fields
/// below are marked with the build that reads them; [`Self::encode`] writes one layout or the
/// other according to the writer's build, and a field belonging to the other build is simply
/// not written. See `documentations/packets-b36731.md` §6 and `tests/handshake_b36731.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerInfo {
    /// The player's own character uuid, as a string. Must match the owner component.
    ///
    /// **10414.** 36731 sends the same identity as a binary uuid in [`Self::owner_uuid`].
    pub owner_guid: String,

    /// Both builds.
    pub owner_name: String,

    /// A `geodata.json > Biomes` name. **10414 only.**
    pub biome: String,

    /// `CRC32` of the adventure name, when the world is one. **10414 only.**
    pub adventure: Option<u32>,

    /// **10414 only.**
    pub map_header_seed: u32,

    /// **10414 only.**
    pub is_home_world: bool,

    /// **10414 only.**
    pub is_my_world: bool,

    /// Both builds.
    pub chat_host: String,

    /// Both builds.
    pub chat_port: u16,

    // --- 36731 only ---------------------------------------------------------------------
    //
    // The uuids are already in the byte order the client reads them in, which is .NET's
    // `Guid.ToByteArray()` (`Uuid::to_bytes_le`), not the RFC 4122 order. Confirmed by hook:
    // the client's own dump showed the same uuids the server sent.
    /// The owner's character uuid. **36731 only.**
    pub owner_uuid: Option<[u8; 16]>,

    /// **36731 only.**
    pub world_uuid: Option<[u8; 16]>,

    /// **36731 only.**
    pub server_uuid: Option<[u8; 16]>,

    /// **36731 only.** Ranged, six bits on the wire — see [`Self::RANGED_USERS_MAX`].
    pub max_users: u32,

    /// **36731 only.** Ranged, six bits.
    pub min_users_required_to_play: u32,

    /// **36731 only.**
    pub game_mode_entity_id: u32,

    /// **36731 only.**
    pub is_opened_to_matchmaking: bool,
}

impl ServerInfo {
    pub const ID: u16 = 58;

    /// The declared maximum behind `MaxUsers` / `MinUsersRequiredToPlay`, giving 6-bit fields.
    ///
    /// The client's reader asks `FUN_00ea7260(0x20)` — a count-leading-zeros — gets 26, and
    /// reads `0x20 - 26` bits. Writing plain 32-bit ints here left the stream 26 bits out per
    /// field and silently garbled everything after them.
    const RANGED_USERS_MAX: u32 = 0x20;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        if writer.build() == ClientBuild::B36731 {
            self.encode_b36731(writer);

            return;
        }

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

    /// The 2017 struct. Field order from the client's deserializer `FUN_007e9c60`.
    ///
    /// The packet id is already written by [`Self::encode`].
    fn encode_b36731(&self, writer: &mut BitWriter) {
        writer.write_optional_bytes(self.owner_uuid.as_ref().map(|uuid| &uuid[..]));
        writer.write_optional_bytes(self.world_uuid.as_ref().map(|uuid| &uuid[..]));
        writer.write_optional_bytes(self.server_uuid.as_ref().map(|uuid| &uuid[..]));

        writer.write_string(&self.owner_name);
        writer.write_string(&self.chat_host);

        writer.write_bits_le(self.max_users, ranged_bits(Self::RANGED_USERS_MAX));
        writer.write_bits_le(
            self.min_users_required_to_play,
            ranged_bits(Self::RANGED_USERS_MAX),
        );

        // Whole-word fields, so RakNet's own big-endian `Write<T>` — not the little-endian
        // ranged idiom two lines above. Both conventions appear in this one packet.
        writer.write_u16(self.chat_port);
        writer.write_u32(self.game_mode_entity_id);

        writer.write_bit(self.is_opened_to_matchmaking);
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
            // 10414's struct has no 2017 fields to read.
            ..Self::default()
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
