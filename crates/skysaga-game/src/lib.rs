//! The RakNet game server.
//!
//! The world handshake, the connection lifecycle, and packet dispatch.
//!
//! # Shape
//!
//! [`Session`] is a **pure state machine**: packets in, packets out, no socket. That is what
//! makes the handshake testable against the C# server's own capture without a client, a
//! network or a running emulator — see `tests/handshake_sequence.rs`, which drives a session
//! with the C#'s world and requires byte-identical output.
//!
//! [`server`] is the thin layer that owns the RakNet peer and moves bytes between it and the
//! sessions.
//!
//! # The handshake
//!
//! Four client packets, each answered with a burst. The client waits at a named loading stage
//! for each burst, so a missing reply shows up as a stall rather than an error.
//!
//! ```text
//! C->S ClientConnected            -> ServerInfo, MapDefinition
//! C->S ClientReadyToSync          -> BeginSync(n), ChunkSync x n
//! C->S ClientInitialSyncFinished  -> EntityAdd x n, ClientEntitiesSyncFinished
//! C->S ClientReadyToPlay          -> SetClientEntity, DebugRequestFinishTutorial
//! ```

pub mod server;
pub mod world;

pub use server::{GameServer, GameServerConfig};
pub use world::{World, WorldConfig};

use std::collections::BTreeSet;

use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::customisation::CustomisationData;
use skysaga_proto::packets::{
    BeginSync, EntityAdd, CharacterCreationResponse, ClientEntitiesSyncFinished, CreateHomeworld,
    DebugRequestFinishTutorial, NotifyPhotoCaptured, PhotoValidated, SaveCharacterName,
    SetCharacterCustomisationData, SetClientEntity, TransferToServer,
};
use tracing::{debug, info, warn};

/// A packet the client sends. Only the ones the server acts on are named.
///
/// Wire id = ordinal + [`ID_USER_PACKET_ENUM`]; these ordinals come from the client's own
/// packet table and were confirmed against a capture of the C# server's handshake.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientPacket {
    /// 135 — the client has connected and wants to know about the world.
    ClientConnected,
    /// 136 — ready to receive terrain.
    ClientReadyToSync,
    /// 137 — ready to be given a body.
    ClientReadyToPlay,
    /// 138 — terrain received; ready for entities.
    ClientInitialSyncFinished,

    /// 242 — the name the player typed. Must be answered or the creator hangs.
    SaveCharacterName(SaveCharacterName),

    /// 244 — sent by the client itself once it accepts `CharacterSaved`.
    CreateHomeworld(CreateHomeworld),

    /// 171 — appearance, sent repeatedly as the creator's options change.
    SetCharacterCustomisation(SetCharacterCustomisationData),

    /// 284 — a photo was taken. Character creation does not finish until this is answered.
    NotifyPhotoCaptured(NotifyPhotoCaptured),

    /// Anything else, by wire id.
    Unknown(u16),
}

impl ClientPacket {
    /// Classify a whole packet, body included.
    ///
    /// Dispatching on the id alone is not enough: `SaveCharacterName` *is* its body, and
    /// losing it means answering the client without knowing what it asked.
    ///
    /// A body that fails to decode falls back to `Unknown` rather than panicking -- these are
    /// bytes from an untrusted peer.
    pub fn parse(bytes: &[u8]) -> Self {
        let mut reader = BitReader::from_bytes(bytes);

        let Ok(id) = reader.read_packet_id() else {
            return Self::Unknown(0);
        };

        let wire_id = id + ID_USER_PACKET_ENUM;

        match id {
            SaveCharacterName::ID => SaveCharacterName::decode(&mut reader)
                .map(Self::SaveCharacterName)
                .unwrap_or(Self::Unknown(wire_id)),

            CreateHomeworld::ID => CreateHomeworld::decode(&mut reader)
                .map(Self::CreateHomeworld)
                .unwrap_or(Self::Unknown(wire_id)),

            SetCharacterCustomisationData::ID => SetCharacterCustomisationData::decode(&mut reader)
                .map(Self::SetCharacterCustomisation)
                .unwrap_or(Self::Unknown(wire_id)),

            NotifyPhotoCaptured::ID => NotifyPhotoCaptured::decode(&mut reader)
                .map(Self::NotifyPhotoCaptured)
                .unwrap_or(Self::Unknown(wire_id)),

            _ => Self::from_wire_id(wire_id),
        }
    }

    /// Classify by *wire* id alone, for the body-less handshake packets.
    pub fn from_wire_id(wire_id: u16) -> Self {
        match wire_id {
            135 => Self::ClientConnected,
            136 => Self::ClientReadyToSync,
            137 => Self::ClientReadyToPlay,
            138 => Self::ClientInitialSyncFinished,
            other => Self::Unknown(other),
        }
    }
}

/// What the player has told us about their character, over RakNet.
///
/// None of it arrives over HTTP: `POST /characters/_create` is posted with an empty body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharacterProfile {
    /// From `SaveCharacterName`.
    pub name: Option<String>,
    /// From `CreateHomeworld` -- a geodata Biome name, never blank.
    pub home_biome: Option<String>,
    /// From `SetCharacterCustomisationData`.
    pub appearance: Option<CustomisationData>,
}

/// How far through the handshake a connection has got.
///
/// Recorded so a stalled client is diagnosable: the stage names which burst the client is
/// waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// RakNet connected; nothing sent yet.
    Connected,
    /// `ServerInfo` and `MapDefinition` sent.
    SentWorldInfo,
    /// Terrain sent.
    SentChunks,
    /// Entities sent.
    SentEntities,
    /// In the world.
    Playing,
}

/// One connected client.
#[derive(Debug)]
pub struct Session {
    stage: Stage,
    player_entity_id: u32,
    character: CharacterProfile,

    /// Entity ids of the items in this player's rucksack, by slot.
    ///
    /// Held per connection rather than in the world: the player body is rebuilt from the
    /// profile, and items are given while the session runs.
    inventory: Vec<u32>,

    /// Which account this connection belongs to.
    ///
    /// Claimed from the conductor's reservation when the connection arrives, because the
    /// connection itself carries no account. `None` for a connection nobody reserved: the
    /// probe and the capture tool connect without going through the conductor.
    account: Option<String>,

    /// Wire ids already reported as unhandled.
    ///
    /// EntityMoved alone arrives dozens of times a minute, so warning per packet buries every
    /// other line. Each id is worth seeing once -- it names a gap in the implementation, and
    /// the second occurrence says nothing the first did not.
    reported: BTreeSet<u16>,
}

impl Session {
    pub fn new(player_entity_id: u32) -> Self {
        Self {
            stage: Stage::Connected,
            player_entity_id,
            character: CharacterProfile::default(),
            inventory: Vec::new(),
            account: None,
            reported: BTreeSet::new(),
        }
    }

    /// The items this player is carrying, by entity id.
    pub fn inventory(&self) -> &[u32] {
        &self.inventory
    }

    /// Put an item entity into the rucksack.
    pub fn take_item(&mut self, entity_id: u32) {
        self.inventory.push(entity_id);
    }

    /// The account this connection belongs to, if one was claimed.
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// Attribute this connection to an account.
    pub fn set_account(&mut self, account: Option<String>) {
        self.account = account;
    }

    /// What the player has told us about their character.
    pub fn character(&self) -> &CharacterProfile {
        &self.character
    }

    /// Seed the session with the character this account already has.
    ///
    /// Character creation ends with the client reconnecting, so on that second connection
    /// the name and appearance exist in storage but have not been sent over this session's
    /// socket. Without seeding them the player would be handed a default-looking body every
    /// time they logged in, and the creator's work would appear to have been discarded.
    ///
    /// Only set fields overwrite: a stored profile never clears something already learnt on
    /// this connection.
    pub fn restore(&mut self, profile: CharacterProfile) {
        if profile.name.is_some() {
            self.character.name = profile.name;
        }

        if profile.home_biome.is_some() {
            self.character.home_biome = profile.home_biome;
        }

        if profile.appearance.is_some() {
            self.character.appearance = profile.appearance;
        }
    }

    /// Wire ids reported as unhandled, in ascending order. Each is reported once.
    pub fn reported_unhandled(&self) -> Vec<u16> {
        self.reported.iter().copied().collect()
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn player_entity_id(&self) -> u32 {
        self.player_entity_id
    }

    /// Handle one client packet, returning the packets to send back, in order.
    ///
    /// Each stage advances only once. A repeated `ClientConnected` is ignored rather than
    /// re-sending the world — resending 16 chunks on demand would be an amplification vector,
    /// and the client does not expect it.
    pub fn handle(&mut self, packet: ClientPacket, world: &World) -> Vec<Vec<u8>> {
        self.handle_with(packet, world, &[])
    }

    /// As [`Self::handle`], also announcing `others`: the bodies of players already here.
    ///
    /// The entity burst is the only place they are needed, and it is the only chance a
    /// joining client gets to learn about them: the client builds its world from this burst
    /// and is told nothing again until something changes.
    pub fn handle_with(
        &mut self,
        packet: ClientPacket,
        world: &World,
        others: &[EntityAdd],
    ) -> Vec<Vec<u8>> {
        match (packet, self.stage) {
            (ClientPacket::ClientConnected, Stage::Connected) => {
                self.stage = Stage::SentWorldInfo;

                info!(stage = ?self.stage, "sending world info");

                vec![
                    encode(|w| world.server_info.encode(w)),
                    encode(|w| world.map.encode(w)),
                ]
            }

            (ClientPacket::ClientReadyToSync, Stage::SentWorldInfo) => {
                self.stage = Stage::SentChunks;

                info!(chunks = world.chunks.len(), "sending terrain");

                let mut out = vec![encode(|w| {
                    BeginSync {
                        chunk_count: world.chunks.len() as u32,
                    }
                    .encode(w)
                })];

                out.extend(world.chunks.iter().map(|chunk| encode(|w| chunk.encode(w))));

                out
            }

            (ClientPacket::ClientInitialSyncFinished, Stage::SentChunks) => {
                self.stage = Stage::SentEntities;

                info!(entities = world.entities.len(), "sending entities");

                // This player's own body, under the id this connection was given, carrying
                // the name and appearance from its profile.
                let player =
                    world.player_body(&self.character, self.player_entity_id, &self.inventory);

                // This player's body goes where the world's template player sits, and the
                // other players are appended.
                //
                // The order is not the client's business, but it is the C#'s: the handshake
                // oracle compares these bytes against a capture of the real server, and
                // moving the player to the end of the burst breaks that comparison for no
                // gain. With one player connected the output is unchanged.
                let mut out: Vec<Vec<u8>> = world
                    .entities
                    .iter()
                    .enumerate()
                    .map(|(index, entity)| {
                        if index == world.player_index {
                            encode(|w| player.encode(w))
                        } else {
                            encode(|w| entity.encode(w))
                        }
                    })
                    .chain(others.iter().map(|entity| encode(|w| entity.encode(w))))
                    .collect();

                out.push(encode(|w| ClientEntitiesSyncFinished.encode(w)));

                out
            }

            (ClientPacket::ClientReadyToPlay, Stage::SentEntities) => {
                self.stage = Stage::Playing;

                info!(entity = self.player_entity_id, "handing over the player entity");

                vec![
                    encode(|w| {
                        SetClientEntity {
                            entity_id: self.player_entity_id,
                        }
                        .encode(w)
                    }),
                    // Without this the client stays in tutorial mode and spills hint text
                    // into the chat log.
                    encode(|w| DebugRequestFinishTutorial.encode(w)),
                ]
            }

            // --- character creation ---------------------------------------------------
            //
            // The client waits on these. Without CharacterSaved it sits on "Creating your
            // character" indefinitely -- observed against this server before these existed.
            (ClientPacket::SaveCharacterName(packet), _) => {
                info!(name = %packet.name, "character named");

                self.character.name = Some(packet.name);

                vec![encode(|w| CharacterCreationResponse::CharacterSaved.encode(w))]
            }

            (ClientPacket::CreateHomeworld(packet), _) => {
                // A blank biome is refused: the client bounces back into the creator on a
                // null homeBiome, so storing one would loop it.
                if packet.home_island_name.trim().is_empty() {
                    warn!("CreateHomeworld carried no biome; not storing it");
                } else {
                    info!(biome = %packet.home_island_name, "homeworld created");

                    self.character.home_biome = Some(packet.home_island_name);
                }

                // ...and then tell it where that homeworld is.
                //
                // Creation ends with the client unloading the creator's world and going
                // idle, still connected, waiting to be told where to go next. Nothing else
                // moves it: the frontend has already finished its own join and will not
                // start another by itself, so without this the client sits on "Waiting for
                // Server" indefinitely.
                //
                // The four fields are the same ones `game-conductor/retrieve` hands out over
                // HTTP -- there is one server, so the client is transferred back to this one.
                vec![
                    encode(|w| CharacterCreationResponse::HomeworldCreated.encode(w)),
                    encode(|w| {
                        TransferToServer {
                            server_uuid: uuid::Uuid::new_v4().to_string(),
                            world_uuid: uuid::Uuid::new_v4().to_string(),
                            ip: world.transfer_ip.clone(),
                            port: world.transfer_port,
                        }
                        .encode(w)
                    }),
                ]
            }

            (ClientPacket::SetCharacterCustomisation(packet), _) => {
                // Sent repeatedly as the creator's options change; the client does not wait
                // on a reply, so there is none.
                debug!(entity = packet.entity_id, "appearance changed");

                self.character.appearance = Some(packet.customisation);

                Vec::new()
            }

            (ClientPacket::NotifyPhotoCaptured(packet), _) => {
                // Answered at *any* stage: the character portrait is captured during
                // creation, before the player is in the world. The client keeps the capture
                // in a pending queue until this reply arrives and will not leave
                // GameState_CharacterCreation without it -- an unanswered capture is an
                // indefinite stall on the "Character Creation" loading screen, with creation
                // itself already successful.
                //
                // The ids are ours to invent; the client only needs them to be distinct and
                // to come back with its own id attached.
                let official_uuid = uuid::Uuid::new_v4().to_string();
                let upload_token = uuid::Uuid::new_v4().to_string();

                info!(
                    photo = packet.client_photo_id,
                    avatar = packet.is_avatar_photo,
                    "photo captured; validating",
                );

                debug!(
                    id = %official_uuid,
                    token = %upload_token,
                    "issued a photo identity",
                );

                vec![encode(|w| {
                    PhotoValidated {
                        client_photo_id: packet.client_photo_id,
                        official_uuid: official_uuid.clone(),
                        upload_token: upload_token.clone(),
                    }
                    .encode(w)
                })]
            }

            (ClientPacket::Unknown(wire_id), _) => {
                // An unimplemented packet is the usual reason a client stalls, so it is worth
                // seeing -- but only once per id. Ordinal is what the documentation tables are
                // keyed by.
                if self.reported.insert(wire_id) {
                    warn!(
                        wire_id,
                        ordinal = wire_id.saturating_sub(ID_USER_PACKET_ENUM),
                        "unhandled client packet (further occurrences silenced)",
                    );
                }

                Vec::new()
            }

            (packet, stage) => {
                // Out of order: a reconnect mid-handshake, a duplicate, or a hostile peer.
                debug!(?packet, ?stage, "ignoring out-of-order packet");

                Vec::new()
            }
        }
    }
}

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}
