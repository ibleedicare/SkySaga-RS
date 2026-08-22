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
use skysaga_proto::packets::movement::{EntityMoved, SetLookAtDirection};
use skysaga_proto::packets::inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferAll, InventoryItemTransferToSlot,
    RequestEquipInventoryItem, RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
};
use skysaga_proto::packets::{
    BeginSync, EntityAdd, EntityRemoved, EntitySync, CharacterCreationResponse,
    ClientEntitiesSyncFinished, CreateHomeworld, DebugRequestFinishTutorial, NotifyPhotoCaptured,
    PhotoValidated, SaveCharacterName, SetCharacterCustomisationData, SetClientEntity,
    TransferToServer,
};
use skysaga_world::inventory::{Effect, Inventories, StackLimits};
use skysaga_world::{Component, Entity};
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

    // --- the rucksack ------------------------------------------------------------------
    //
    // The client applies none of these locally: it sends the request and waits for the
    // inventory to be synced back. An unhandled one is a UI that appears to freeze
    // mid-drag rather than an error, which is why they are worth naming individually.
    /// 185 — a drop onto an empty square: move, or split.
    InventoryItemTransferToSlot(InventoryItemTransferToSlot),

    /// 187 — a drop onto an occupied square: merge, or exchange.
    InventoryItemSwap(InventoryItemSwap),

    /// 186 — the loot window's "Take All".
    InventoryItemTransferAll(InventoryItemTransferAll),

    /// 179 — the rucksack's trash can.
    InventoryItemDestroy(InventoryItemDestroy),

    /// 147 — equip something from the rucksack.
    RequestEquipInventoryItem(RequestEquipInventoryItem),

    /// 149 — bind an item to a hotbar square.
    RequestUiSettingsSlotChange(RequestUiSettingsSlotChange),

    /// 150 — select a different hotbar square.
    RequestUiSettingsSetActiveSlot(RequestUiSettingsSetActiveSlot),

    // --- where the player is ------------------------------------------------------------
    //
    // Neither is answered: the client has already moved itself and is not waiting to be told
    // it may. They are decoded because the *server* needs the answers, and because
    // `EntityMoved` arrives dozens of times a minute -- as an unhandled packet it was the
    // loudest line in the log, which is the noise that hides a real gap.
    /// 236 — a player is here now.
    EntityMoved(EntityMoved),

    /// 240 — a player is looking that way.
    SetLookAtDirection(SetLookAtDirection),

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

            InventoryItemTransferToSlot::ID => InventoryItemTransferToSlot::decode(&mut reader)
                .map(Self::InventoryItemTransferToSlot)
                .unwrap_or(Self::Unknown(wire_id)),

            InventoryItemSwap::ID => InventoryItemSwap::decode(&mut reader)
                .map(Self::InventoryItemSwap)
                .unwrap_or(Self::Unknown(wire_id)),

            InventoryItemTransferAll::ID => InventoryItemTransferAll::decode(&mut reader)
                .map(Self::InventoryItemTransferAll)
                .unwrap_or(Self::Unknown(wire_id)),

            InventoryItemDestroy::ID => InventoryItemDestroy::decode(&mut reader)
                .map(Self::InventoryItemDestroy)
                .unwrap_or(Self::Unknown(wire_id)),

            RequestEquipInventoryItem::ID => RequestEquipInventoryItem::decode(&mut reader)
                .map(Self::RequestEquipInventoryItem)
                .unwrap_or(Self::Unknown(wire_id)),

            RequestUiSettingsSlotChange::ID => RequestUiSettingsSlotChange::decode(&mut reader)
                .map(Self::RequestUiSettingsSlotChange)
                .unwrap_or(Self::Unknown(wire_id)),

            RequestUiSettingsSetActiveSlot::ID => {
                RequestUiSettingsSetActiveSlot::decode(&mut reader)
                    .map(Self::RequestUiSettingsSetActiveSlot)
                    .unwrap_or(Self::Unknown(wire_id))
            }

            EntityMoved::ID => EntityMoved::decode(&mut reader)
                .map(Self::EntityMoved)
                .unwrap_or(Self::Unknown(wire_id)),

            SetLookAtDirection::ID => SetLookAtDirection::decode(&mut reader)
                .map(Self::SetLookAtDirection)
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

    /// Every inventory this connection can address, and the item entities in them.
    ///
    /// Indexed by the slot layout in [`crate::world::MAX_INVENTORY_SLOTS`]: worn and held
    /// items first, the rucksack from slot 9.
    ///
    /// Held per connection rather than in the world because the only inventory a connection
    /// can reach today is its own player's -- the body is rebuilt from the profile, and items
    /// are given while the session runs. A **shared** container, which is what a chest is,
    /// will not fit here: two players looking into one chest have to see one set of contents,
    /// so opening containers means moving this to the server and passing it in.
    inventories: Inventories,

    /// Hotbar square to the item name hash bound to it.
    ///
    /// Not storage. `hotbarslotresources` holds *resources*, so a bound stack stays in the
    /// rucksack; this is only the server's record of what the player is holding.
    hotbar: std::collections::HashMap<u32, u32>,

    /// Which hotbar square is selected.
    active_slot: u32,

    /// Where the client last said this player is, in the client's own units.
    ///
    /// `None` until it says so. Not defaulted to the spawn point: anything reading this has
    /// to tell "standing at the origin" from "has not reported yet", and a default makes
    /// those the same value.
    position: Option<[u32; 3]>,

    /// Which way the player is facing, from the same packet.
    facing_yaw: Option<u32>,

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
        // Item entities are minted from just past the player's own id. The game server hands
        // out ids globally and keeps this in step around every packet it dispatches (see
        // `reserve_ids_from` / `next_entity_id`), so this base only matters to a session
        // driven directly, as the tests do.
        let mut inventories = Inventories::new(StackLimits::default(), player_entity_id + 1);

        inventories.open(player_entity_id, crate::world::MAX_INVENTORY_SLOTS as usize);

        Self {
            stage: Stage::Connected,
            player_entity_id,
            character: CharacterProfile::default(),
            inventories,
            hotbar: std::collections::HashMap::new(),
            active_slot: 0,
            position: None,
            facing_yaw: None,
            account: None,
            reported: BTreeSet::new(),
        }
    }

    /// The items this player is carrying, by entity id and slot.
    pub fn inventory(&self) -> &[u32] {
        self.inventories.slots(self.player_entity_id)
    }

    /// Read-only access to the model, for serialising an item entity.
    pub fn inventories(&self) -> &Inventories {
        &self.inventories
    }

    /// What is in one of the player's slots. `Some(0)` for empty, `None` for no such slot.
    pub fn slot(&self, slot: u32) -> Option<u32> {
        self.inventories.slot(self.player_entity_id, slot)
    }

    /// Which of the player's slots holds `entity`.
    pub fn slot_of(&self, entity: u32) -> Option<u32> {
        self.inventory()
            .iter()
            .position(|held| *held == entity)
            .map(|slot| slot as u32)
    }

    /// Where the client last said this player is, or `None` if it has not said yet.
    pub fn position(&self) -> Option<[u32; 3]> {
        self.position
    }

    /// Which way the player is facing, or `None` if the client has not said yet.
    pub fn facing_yaw(&self) -> Option<u32> {
        self.facing_yaw
    }

    /// The item hash bound to the selected hotbar square.
    ///
    /// What the player is holding. Placing a block and digging arrive as the same
    /// `PerformVoxelActions` packet, and this is what tells the two apart.
    pub fn held_resource(&self) -> Option<u32> {
        self.hotbar.get(&self.active_slot).copied()
    }

    /// Create a stack of `item` in the first free rucksack square, returning its entity id.
    ///
    /// `None` when the rucksack is full. Slots below
    /// [`FIRST_RUCKSACK_SLOT`](crate::world::FIRST_RUCKSACK_SLOT) are what the player is
    /// wearing or holding, so filling those from here would silently equip things.
    pub fn give(&mut self, item: &str, count: u32) -> Option<u32> {
        let slot = self.inventories.first_free_rucksack_slot(self.player_entity_id)?;

        self.inventories
            .give(self.player_entity_id, slot, skysaga_core::name_hash(item), count)
    }

    /// Mint item entities from `next` upwards.
    ///
    /// The game server allocates entity ids globally, and calls this before dispatching a
    /// packet so a split stack cannot collide with another connection's body.
    pub fn reserve_ids_from(&mut self, next: u32) {
        self.inventories.reserve_ids_from(next);
    }

    /// The next entity id this session would mint. The inverse of [`Self::reserve_ids_from`].
    pub fn next_entity_id(&self) -> u32 {
        self.inventories.next_entity_id()
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
                    world.player_body(&self.character, self.player_entity_id, self.inventory());

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

            // --- the rucksack ---------------------------------------------------------
            //
            // Each of these is: decode (already done), call the model, turn the effects it
            // reports into packets. Nothing about which stack merges into which lives here;
            // that is all in `skysaga-world::inventory`, where it is testable without a
            // socket.
            (ClientPacket::InventoryItemTransferToSlot(packet), _) => {
                let effects = self.inventories.transfer_to_slot(
                    packet.source_entity,
                    packet.source_slot,
                    packet.target_entity,
                    packet.target_slot,
                    packet.count,
                );

                self.apply(effects, world)
            }

            (ClientPacket::InventoryItemSwap(packet), _) => {
                let effects = self.inventories.swap(
                    packet.source_entity,
                    packet.source_slot,
                    packet.target_entity,
                    packet.target_slot,
                );

                self.apply(effects, world)
            }

            (ClientPacket::InventoryItemTransferAll(packet), _) => {
                let effects = self
                    .inventories
                    .transfer_all(packet.source_entity, packet.target_entity);

                self.apply(effects, world)
            }

            (ClientPacket::InventoryItemDestroy(packet), _) => {
                let effects = self
                    .inventories
                    .destroy(packet.entity_id, packet.slot, packet.count);

                self.apply(effects, world)
            }

            (ClientPacket::RequestEquipInventoryItem(packet), _) => {
                let effects =
                    self.inventories
                        .equip(packet.entity_id, packet.bag_slot, packet.equip_slot);

                // The hands are a hotbar bind, not a move, and the model reports no effects
                // for them. Record what is now held, which is the whole point of the packet.
                if packet.equip_slot < 2 {
                    self.hotbar.remove(&self.active_slot);

                    if let Some(item) = self
                        .inventories
                        .slot(packet.entity_id, packet.bag_slot)
                        .filter(|item| *item != 0)
                        .and_then(|item| self.inventories.name(item))
                    {
                        self.hotbar.insert(self.active_slot, item);
                    }
                }

                self.apply(effects, world)
            }

            (ClientPacket::RequestUiSettingsSlotChange(packet), _) => {
                debug!(slot = packet.slot, resource = packet.resource, "hotbar bound");

                self.hotbar.insert(packet.slot, packet.resource);

                // A fresh bind is also what the player just selected: the client does not
                // always follow one with a SetActiveSlot.
                self.active_slot = packet.slot;

                // Deliberately nothing back. `hotbarslotresources` (sync index 34) is kept by
                // the client itself, and its encoding is not confirmed -- echoing a wrong one
                // would draw a wrong hotbar, which is worse than drawing the client's own.
                Vec::new()
            }

            (ClientPacket::RequestUiSettingsSetActiveSlot(packet), _) => {
                debug!(slot = packet.slot, "hotbar square selected");

                self.active_slot = packet.slot;

                Vec::new()
            }

            // --- where the player is ---------------------------------------------------
            (ClientPacket::EntityMoved(packet), _) => {
                // Only about this connection's own body. A client claiming to move another
                // player's entity must not change what this session believes about itself;
                // relaying the bytes on is the server layer's business and is unaffected.
                if packet.entity_id == self.player_entity_id {
                    self.position = Some(packet.position);
                    self.facing_yaw = Some(packet.yaw);
                }

                Vec::new()
            }

            (ClientPacket::SetLookAtDirection(packet), _) => {
                // Decoded and dropped, as in the C#. Nothing reads a look direction yet; the
                // value of handling it is that it stops burying the log.
                debug!(?packet, "look direction");

                Vec::new()
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

impl Session {
    /// Turn the model's [`Effect`]s into packets, in the order they must be sent.
    ///
    /// The ordering is the model's, not this function's: an `ItemCreated` arrives before the
    /// `SlotsChanged` that points a slot at it, because a slot naming an entity the client has
    /// never been told about draws an empty square.
    ///
    /// An effect that cannot be turned into a packet is dropped with a warning rather than
    /// panicking. The two ways that happens are a data file with no `BasicInventoryItem`, and
    /// an inventory belonging to something other than this player -- which is what a chest
    /// will be, and is the extension point when containers arrive.
    fn apply(&mut self, effects: Vec<Effect>, world: &World) -> Vec<Vec<u8>> {
        effects
            .into_iter()
            .filter_map(|effect| self.packet_for(effect, world))
            .collect()
    }

    fn packet_for(&self, effect: Effect, world: &World) -> Option<Vec<u8>> {
        match effect {
            Effect::ItemCreated { entity } => {
                let (item, definition) = self.item_entity(entity, world)?;

                Some(encode(|w| item.to_entity_add(definition).encode(w)))
            }

            Effect::ItemChanged { entity } => {
                let (item, definition) = self.item_entity(entity, world)?;

                // Only the parameter that changed. The client is never sent a full update for
                // an entity it already holds, and sending one is not what the C# does.
                let sync_data = item.sync_data_for(definition, &["inventoryslotdata"]);

                Some(encode(|w| {
                    let mut payload = BitWriter::new();
                    sync_data.encode(&mut payload);

                    EntitySync {
                        id: entity,
                        sync_data: skysaga_proto::packets::Bits::from_writer(&payload),
                    }
                    .encode(w)
                }))
            }

            Effect::ItemRemoved { entity } => {
                Some(encode(|w| EntityRemoved { entity_id: entity }.encode(w)))
            }

            Effect::SlotsChanged { owner } => {
                if owner != self.player_entity_id {
                    // A container. Nothing in the world has one yet, so there is no definition
                    // to encode it from; this is where opening a chest will plug in.
                    warn!(owner, "no definition for this inventory; not syncing it");

                    return None;
                }

                let (player, definition) =
                    world.player_entity(&self.character, owner, self.inventory())?;

                let sync_data = player.sync_data_for(definition, &["inventoryentitylist"]);

                Some(encode(|w| {
                    let mut payload = BitWriter::new();
                    sync_data.encode(&mut payload);

                    EntitySync {
                        id: owner,
                        sync_data: skysaga_proto::packets::Bits::from_writer(&payload),
                    }
                    .encode(w)
                }))
            }
        }
    }

    /// One item entity, ready to serialise.
    fn item_entity<'a>(
        &self,
        entity: u32,
        world: &'a World,
    ) -> Option<(Entity, &'a skysaga_world::EntityDefinition)> {
        let definition = world.item_definition().or_else(|| {
            warn!("BasicInventoryItem is not defined; cannot serialise a stack");

            None
        })?;

        let component = self.inventories.item(entity)?;

        Some((
            Entity::new(entity, vec![Component::InventoryItem(component.clone())]),
            definition,
        ))
    }
}

fn encode(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}
