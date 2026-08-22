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
use skysaga_proto::packets::chat::{RequestChatChannelData, SendChatChannelData};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction, InteractWithEntity};
use skysaga_proto::packets::mail::{
    DeleteMail, MailCheck, MailGiftSelected, MailRead, NewMailReceived, RemoteMailSynced,
    TakeMailAttachment,
};
use skysaga_proto::packets::movement::{EntityMoved, SetLookAtDirection};
use skysaga_proto::packets::voxel::{ChunkEdit, PartialChunkEditsSync, PerformVoxelActions};
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

/// How many squares a mail attachment container declares.
///
/// The client's own `MailItem` container is five, take-only.
pub const MAIL_ATTACHMENT_SLOTS: usize = 5;

/// Where the mail UI starts reading a container's inventory.
///
/// **Not zero.** The panel takes the attachment list from index 9 and derives the count from
/// `maxinventoryslots` minus the empties from there on, so the list has to be
/// `MAIL_ATTACHMENT_SLOTS + 9` entries with the attachments at 9 and up. Filling 0..4 of a
/// five-entry list rendered nothing and walked the client off the end of the array -- three
/// sessions were spent on the wire format before the layout turned out to be the problem.
pub const MAIL_ATTACHMENT_BASE: usize = 9;

/// How many dig ticks break a block.
///
/// From the C#. The client streams one packet per tick and every field is identical across
/// the run, so the count is the server's to keep.
pub const DIG_TICKS_TO_BREAK: u32 = 3;

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

    // --- doing something to an entity ---------------------------------------------------
    /// 198 — who did what to what. **Pressing E arrives here**, not as `InteractWithEntity`.
    ExecuteEntityAction(ExecuteEntityAction),

    /// 154 — who touched what. Carries no verb, so nothing can be decided from it.
    InteractWithEntity(InteractWithEntity),

    /// 151 — a block was placed or broken. Every build action ends up here.
    PerformVoxelActions(PerformVoxelActions),

    // --- the mailbox --------------------------------------------------------------------
    //
    // All the mail data goes the other way, as `Player.mailitemlist` inside an ordinary
    // entity sync. These are requests carrying at most two strings.
    /// 230 — "send me my inbox". No body; the id is the whole message.
    MailCheck(MailCheck),

    /// 228 — a message was opened.
    MailRead(MailRead),

    /// 229 — a gift option was picked. Does not say which.
    MailGiftSelected(MailGiftSelected),

    /// 231 — discard a message.
    DeleteMail(DeleteMail),

    /// 225 — "which channels are there?". No body.
    RequestChatChannelData(RequestChatChannelData),

    /// 232 — claim an attachment into the rucksack.
    TakeMailAttachment(TakeMailAttachment),

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

            ExecuteEntityAction::ID => ExecuteEntityAction::decode(&mut reader)
                .map(Self::ExecuteEntityAction)
                .unwrap_or(Self::Unknown(wire_id)),

            InteractWithEntity::ID => InteractWithEntity::decode(&mut reader)
                .map(Self::InteractWithEntity)
                .unwrap_or(Self::Unknown(wire_id)),

            PerformVoxelActions::ID => PerformVoxelActions::decode(&mut reader)
                .map(Self::PerformVoxelActions)
                .unwrap_or(Self::Unknown(wire_id)),

            MailCheck::ID => MailCheck::decode(&mut reader)
                .map(Self::MailCheck)
                .unwrap_or(Self::Unknown(wire_id)),

            MailRead::ID => MailRead::decode(&mut reader)
                .map(Self::MailRead)
                .unwrap_or(Self::Unknown(wire_id)),

            MailGiftSelected::ID => MailGiftSelected::decode(&mut reader)
                .map(Self::MailGiftSelected)
                .unwrap_or(Self::Unknown(wire_id)),

            DeleteMail::ID => DeleteMail::decode(&mut reader)
                .map(Self::DeleteMail)
                .unwrap_or(Self::Unknown(wire_id)),

            RequestChatChannelData::ID => RequestChatChannelData::decode(&mut reader)
                .map(Self::RequestChatChannelData)
                .unwrap_or(Self::Unknown(wire_id)),

            TakeMailAttachment::ID => TakeMailAttachment::decode(&mut reader)
                .map(Self::TakeMailAttachment)
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

    /// The entity whose container this player has open, or 0 for none.
    ///
    /// **This is the whole opening mechanism.** There is no "open the container" packet: the
    /// client opens a loot window when the *player's* `usingentityid` becomes the target's id,
    /// and closes it when that goes back to 0.
    using_entity: u32,

    /// Voxels this player has changed: (chunk, voxel) to the new material.
    ///
    /// Held per connection, as containers are, so a block one player places is not in another
    /// player's world. The same limitation, and it moves at the same time.
    voxel_edits: std::collections::HashMap<([u32; 3], [u32; 3]), u8>,

    /// Containers spawned while this session runs, beside the ones the world seeded.
    ///
    /// Per session, as the seeded containers effectively are: a chest one player spawns is
    /// not in another player's world. The same limitation, and it moves at the same time.
    spawned: Vec<world::Container>,

    /// This player's inbox.
    mailbox: Vec<Mail>,

    /// Packets to send that no client packet asked for -- the mail doorbell, so far.
    notifications: Vec<Vec<u8>>,

    /// Dig ticks accumulated per voxel, until it gives way.
    dig_damage: std::collections::HashMap<([u32; 3], [u32; 3]), u32>,

    /// Containers whose `hasbeenopened` is currently raised.
    ///
    /// The **close** signal, not the open one. The client's open path fires only while it is
    /// clear and its close path on the rising edge, so it is raised to shut a lid and lowered
    /// again before the next open.
    closed_lids: BTreeSet<u32>,

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
            using_entity: 0,
            closed_lids: BTreeSet::new(),
            voxel_edits: std::collections::HashMap::new(),
            dig_damage: std::collections::HashMap::new(),
            spawned: Vec::new(),
            mailbox: Vec::new(),
            notifications: Vec::new(),
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

    /// Create a stack of `item` in one particular square, for tests and for seeding.
    pub fn give_at(&mut self, slot: u32, item: &str, count: u32) -> Option<u32> {
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

                // Stack limits come from the game's own table, not from the default of 64:
                // fourteen items override it, and a limit that is merely assumed silently
                // loses the overflow when two stacks merge.
                self.inventories.set_limits(world.geodata.stack_limits());

                // Give every container in the world an inventory in this session's model, so
                // a drag into a chest has somewhere to land. Done here rather than in `new`
                // because the world is not known until a packet arrives.
                for container in &world.containers {
                    if !self.inventories.is_open(container.id) {
                        self.inventories.open(container.id, container.slots);
                    }
                }

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

            // --- doing something to an entity ------------------------------------------
            (ClientPacket::ExecuteEntityAction(packet), _) => {
                debug!(
                    source = packet.source_entity,
                    target = packet.target_entity,
                    action = ?packet.action,
                    "entity action",
                );

                // Only Interact opens anything. Opening on any action at all would have a
                // pickaxe swing open the loot window.
                if packet.action == Some(Action::Interact) {
                    self.open_container(packet.target_entity, world)
                } else {
                    Vec::new()
                }
            }

            (ClientPacket::InteractWithEntity(packet), _) => {
                // No verb, so there is nothing to decide. Named rather than left to `Unknown`
                // because it is sent alongside every E press, and an unhandled packet that
                // arrives on every interaction is noise that hides real gaps.
                debug!(
                    interacting = packet.interacting_entity,
                    target = packet.target_entity,
                    "interact",
                );

                Vec::new()
            }

            // --- building and digging ----------------------------------------------------
            (ClientPacket::PerformVoxelActions(packet), _) => self.perform_voxel_action(packet, world),

            // --- chat ---------------------------------------------------------------------
            (ClientPacket::RequestChatChannelData(_), _) => {
                // The channel list, and nothing else: every actual message goes over the IRC
                // socket. Without this reply the client never issues a JOIN, so the chat
                // server sits with a registered but silent client -- which looks exactly like
                // an IRC server that is down.
                info!(channels = world.chat_channels.len(), "sending the channel list");

                vec![encode(|w| {
                    SendChatChannelData {
                        channels: world.chat_channels.clone(),
                        trailing: [String::new(), String::new()],
                    }
                    .encode(w)
                })]
            }

            // --- the mailbox --------------------------------------------------------------
            (ClientPacket::MailCheck(_), _) => self.sync_mailbox(world),

            (ClientPacket::MailRead(packet), _) => {
                // No reply: the client set its own read bit before sending. Recording it is
                // what stops the next re-sync popping the message back to unread.
                if let Some(mail) = self.mail_mut(&packet.message_uuid) {
                    mail.set_read();

                    debug!(uuid = %packet.message_uuid, "mail read");
                }

                Vec::new()
            }

            (ClientPacket::MailGiftSelected(packet), _) => {
                if let Some(mail) = self.mail_mut(&packet.message_uuid) {
                    mail.set_gift_chosen();

                    debug!(uuid = %packet.message_uuid, "mail gift chosen");
                }

                Vec::new()
            }

            (ClientPacket::DeleteMail(packet), _) => {
                let before = self.mailbox.len();

                self.mailbox.retain(|mail| mail.uuid != packet.message_uuid);

                if self.mailbox.len() == before {
                    return Vec::new();
                }

                debug!(uuid = %packet.message_uuid, "mail deleted");

                // The client does not remove the row itself; it goes when the list comes back
                // without it.
                self.sync_mailbox(world)
            }

            (ClientPacket::TakeMailAttachment(packet), _) => {
                self.claim_attachment(&packet.message_uuid, &packet.item_uuid, world)
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
    /// The container with this entity id, whether the world seeded it or a command spawned it.
    ///
    /// **The session first.** The world is fixed once built, so anything created while the
    /// server runs lives here; checking only the world makes every spawned chest inert, and
    /// checking only the session breaks the seeded one.
    pub fn container<'a>(&'a self, id: u32, world: &'a World) -> Option<&'a world::Container> {
        self.spawned
            .iter()
            .find(|container| container.id == id)
            .or_else(|| world.container(id))
    }

    /// Put a chest in the world, in front of the player.
    ///
    /// `loot` entries are `Item` or `Item:count`. Returns the entity, where it went, and the
    /// packets that announce it -- **the loot before the chest**, because the chest's slot
    /// list names those entities and a slot pointing at one the client has not been told about
    /// draws an empty square.
    ///
    /// `None` when the data file defines no such entity: the name comes from a chat message,
    /// so it is whatever somebody typed.
    pub fn spawn_chest(&mut self, world: &World, entity: &str, loot: &[&str]) -> Option<Spawned> {
        let definition = world.definitions.get(entity)?.clone();

        let id = self.inventories.next_entity_id();
        self.inventories.reserve_ids_from(id + 1);

        let position = self.spawn_position(world);

        let links = definition
            .default_voxel_links()
            .into_iter()
            .map(|(offset, voxel_index)| skysaga_world::VoxelLink {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                voxel_index,
            })
            .collect();

        let built = Entity::new(
            id,
            world::container_components(position, links, world::CHEST_SLOTS),
        );

        self.inventories.open(id, world::CHEST_SLOTS);

        // The loot, in the chest's own squares.
        let mut packets = Vec::new();

        for (slot, entry) in loot.iter().enumerate().take(world::CHEST_SLOTS) {
            let (name, count) = match entry.split_once(':') {
                Some((name, count)) => (name, count.parse().unwrap_or(1)),
                None => (*entry, 1),
            };

            let Some(item) =
                self.inventories
                    .give(id, slot as u32, skysaga_core::name_hash(name), count)
            else {
                continue;
            };

            self.inventories.reserve_ids_from(self.inventories.next_entity_id());

            // Announced first, so the chest's slot list never names an unknown entity.
            if let Some((stack, item_definition)) = self.item_entity(item, world) {
                packets.push(encode(|w| {
                    stack.to_entity_add(item_definition).encode(w)
                }));
            }
        }

        let container = world::Container {
            id,
            name: entity.to_owned(),
            entity: built,
            definition,
            slots: world::CHEST_SLOTS,
            is_loot_chest: true,
        };

        self.spawned.push(container);

        // ...and now the chest, rebuilt through `entity_now` so its slot list carries the loot.
        if let Some((built, definition)) = self.entity_now(id, world) {
            packets.push(encode(|w| built.to_entity_add(definition).encode(w)));
        }

        info!(entity = id, %entity, loot = loot.len(), ?position, "spawned a chest");

        Some(Spawned {
            entity: id,
            position,
            packets,
        })
    }

    /// Three voxels in front of the player, or the world's spawn point if it has not moved.
    ///
    /// The facing is the raw value from `EntityMoved`, whose units are **not confirmed**: the
    /// C# reads that field as a float and gets a denormal, so its own chests always land due
    /// north whatever the player is doing. A full circle is assumed to be the field's declared
    /// maximum. If that is wrong the chest appears on a different side of the player, three
    /// voxels away either way, which is close enough to press E on while it stays unproven.
    fn spawn_position(&self, world: &World) -> [u32; 3] {
        const DISTANCE: f32 = 3.0 * world::POSITION_SCALE as f32;

        /// The declared maximum of the yaw field, taken as a full turn.
        const FULL_TURN: f32 = 25_600.0;

        let Some(position) = self.position else {
            // The client has not said where it is yet. The world's spawn point is at least on
            // the island, where the origin is buried in terrain.
            let spawn = world.spawn_position();

            return [spawn[0], spawn[1], spawn[2] + world::POSITION_SCALE * 3];
        };

        let turns = self.facing_yaw.unwrap_or(0) as f32 / FULL_TURN;
        let radians = turns * std::f32::consts::TAU;

        [
            position[0].saturating_add_signed((radians.sin() * DISTANCE).round() as i32),
            position[1],
            position[2].saturating_add_signed((radians.cos() * DISTANCE).round() as i32),
        ]
    }

    /// Open or close the container `target`, and say what to send.
    ///
    /// # There is no "open the container" packet
    ///
    /// The client opens a loot window when the **player's** `usingentityid` becomes the
    /// target's entity id, and closes it when that goes back to 0. Nothing is sent to the
    /// container to open it; the answer to an interact is a sync of a component on the
    /// *player*. Years of adjusting chest parameters got nowhere because of this.
    ///
    /// # `hasbeenopened` is the close signal
    ///
    /// The client's open path fires only while it is clear, and its close path on the
    /// false -> true edge. So it is raised to shut a lid and lowered again before the next
    /// open, which is safe because the two happen on separate key presses.
    ///
    /// # Why a loot chest toggles and nothing else does
    ///
    /// **The client never says when a panel is dismissed.** Clicking a window's X sends
    /// nothing at all, and the interact fires on open only. So a plain toggle drifts out of
    /// phase the moment a player closes a window with the mouse: the server still believes it
    /// open, the next E is spent "closing" something already gone, and the player has to press
    /// E twice.
    ///
    /// A loot chest is the exception -- it has no close button, so E really is the only way to
    /// shut it. Anything with an X is re-opened instead: `usingentityid` drops to 0 and is
    /// restored, because the client reacts to a *change* and would ignore being told the same
    /// id twice.
    fn open_container(&mut self, target: u32, world: &World) -> Vec<Vec<u8>> {
        let Some(container) = self.container(target, world).cloned() else {
            debug!(target, "not a container; nothing to open");

            return Vec::new();
        };

        let opening = self.using_entity != target;

        if !opening && !container.is_loot_chest {
            // Re-open. Two syncs rather than one: the client ignores being told the id it
            // already holds, so it has to see 0 and then the id again.
            //
            // The C# spreads these over two ticks. Here they are two packets in one burst,
            // which is two distinct values in the order they must be read. Worth confirming
            // in the client the first time a panel with an X button is wired up; the chest
            // path below does not use it.
            self.closed_lids.remove(&target);

            let mut out = Vec::new();

            self.using_entity = 0;
            out.extend(self.sync_player(world));

            self.using_entity = target;
            out.extend(self.sync_player(world));

            debug!(target, "re-opening; the client closed it without telling us");

            return out;
        }

        self.using_entity = if opening { target } else { 0 };

        if opening {
            self.closed_lids.remove(&target);
        } else {
            self.closed_lids.insert(target);
        }

        debug!(target, opening, "container");

        if opening {
            // Only the player. `hasbeenopened` is already false, so there is no edge to send
            // and the C# -- which syncs what changed and nothing else -- sends nothing here.
            return self.sync_player(world);
        }

        // **The lid before the window, and this order is load-bearing.**
        //
        // `usingentityid` going to 0 closes the loot *window*. `hasbeenopened` going
        // false -> true plays the *lid* animation, and the client fires that only while its own
        // "window open" latch is still set. Closing the window first clears the latch, so the
        // `hasbeenopened` edge lands too late and is dropped -- the window goes, the chest
        // stays standing open, and nothing reports an error.
        let mut out = self.sync_container(target, world);

        out.extend(self.sync_player(world));

        out
    }

    /// Place a block or break one, and say what to send.
    ///
    /// **Place and break are the same packet.** What tells them apart is only whether the
    /// slot that acted is a hand holding a placeable block; anything else -- a tool, an empty
    /// hand, a hit from the torso -- digs. Before that distinction existed in the C#, swinging
    /// an anvil at the ground broke the block.
    ///
    /// The client predicts the change locally and waits to be told it really happened, so an
    /// unanswered dig is a block that vanishes and comes back. That reads as lag rather than
    /// as a missing handler, which is why this is worth answering even though nothing else
    /// depends on it yet.
    fn perform_voxel_action(&mut self, packet: PerformVoxelActions, world: &World) -> Vec<Vec<u8>> {
        // Only a hand can be holding anything, and only a placeable block places. The hotbar
        // keeps resource *hashes*, so it can still name an item the player has run out of --
        // hence the check that a stack actually exists before one is taken from it.
        let placing = packet.location.is_hand().then(|| self.held_block(world)).flatten();

        let Some((item, material)) = placing else {
            return self.dig(packet.chunk, packet.voxel);
        };

        // Taking from the stack also confirms there was one to take.
        if !self.take_one(item) {
            debug!(item, "the hotbar names a block the player does not have");

            return self.dig(packet.chunk, packet.voxel);
        }

        // The new block goes into the empty voxel next to the face that was clicked, not into
        // the one that was hit.
        let voxel = packet.placement_voxel();

        self.voxel_edits.insert((packet.chunk, voxel), material);

        debug!(?packet.chunk, ?voxel, material, "place");

        vec![Self::chunk_edit(packet.chunk, voxel, material)]
    }

    // --- the mailbox --------------------------------------------------------------------

    /// Every message in this player's inbox.
    pub fn mailbox(&self) -> &[Mail] {
        &self.mailbox
    }

    /// One message, by uuid.
    pub fn mail(&self, uuid: &str) -> Option<&Mail> {
        self.mailbox.iter().find(|mail| mail.uuid == uuid)
    }

    fn mail_mut(&mut self, uuid: &str) -> Option<&mut Mail> {
        self.mailbox.iter_mut().find(|mail| mail.uuid == uuid)
    }

    /// Put a message in this player's inbox, with `attachments` as real item entities.
    ///
    /// Returns its uuid. The doorbell packet is queued rather than returned, because composing
    /// is not something the client asked for: it happens on an admin command or a server
    /// event, with no packet of the player's to answer. See [`Self::take_notifications`].
    pub fn compose(&mut self, subject: &str, body: &str, attachments: &[(&str, u32)]) -> String {
        let uuid = self.next_uuid();

        // The attachment container is an entity like any other, and the items in it are item
        // entities like any other -- exactly as a chest holds loot.
        let container = self.inventories.next_entity_id();
        self.inventories.reserve_ids_from(container + 1);

        self.inventories
            .open(container, MAIL_ATTACHMENT_SLOTS + MAIL_ATTACHMENT_BASE);

        for (index, (item, count)) in attachments.iter().enumerate().take(MAIL_ATTACHMENT_SLOTS) {
            self.inventories.give(
                container,
                (MAIL_ATTACHMENT_BASE + index) as u32,
                skysaga_core::name_hash(item),
                *count,
            );
        }

        self.mailbox.push(Mail {
            uuid: uuid.clone(),
            subject: subject.to_owned(),
            body: body.to_owned(),
            attachment_entity: container,
            flags: 0,
        });

        info!(%uuid, subject, attachments = attachments.len(), "mail composed");

        // The doorbell. Its handler does nothing but send MailCheck back, which is how a
        // message arriving while the panel is shut still lights the icon.
        self.notifications.push(encode(|w| {
            NewMailReceived {
                message_uuid: uuid.clone(),
            }
            .encode(w)
        }));

        uuid
    }

    /// Packets the server should send that no client packet asked for.
    ///
    /// Draining, so a notification goes out once. `Session::handle` answers a request; this is
    /// how something that happens *to* a player reaches them.
    pub fn take_notifications(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.notifications)
    }

    /// Send the inbox, then say it is complete.
    ///
    /// **Both packets, in that order.** The client's panel renders its loading state and draws
    /// no rows until `RemoteMailSynced` arrives -- even when `mailitemlist` was synced
    /// perfectly. The channel is reliable-ordered, so sending them in order is enough.
    ///
    /// The attachment containers are deliberately **not** re-announced here. A repeat
    /// `EntityAdd` for an id the client holds makes it destroy the entity and build a fresh
    /// one, leaving every slot list that still names the old object holding a dangling
    /// pointer -- and that pointer is the one the client's contents-recompute dereferences.
    /// Announce once, at compose time, and let later changes ride `EntitySync`.
    fn sync_mailbox(&mut self, world: &World) -> Vec<Vec<u8>> {
        let mut out = Vec::new();

        if let Some(sync) = self.sync_of(self.player_entity_id, &["mailitemlist"], world) {
            out.push(sync);
        }

        out.push(encode(|w| RemoteMailSynced.encode(w)));

        debug!(messages = self.mailbox.len(), "mailbox synced");

        out
    }

    /// Move one attachment into the rucksack.
    ///
    /// The client identifies the item by uuid rather than by slot, and has already blanked its
    /// own copy of that square -- so doing nothing leaves the item *nowhere* until a re-bind
    /// restores it. Every path here therefore re-syncs, including the failures.
    fn claim_attachment(&mut self, message: &str, item_uuid: &str, world: &World) -> Vec<Vec<u8>> {
        let Some(container) = self.mail(message).map(|mail| mail.attachment_entity) else {
            return Vec::new();
        };

        let Some(source) = (0..self.inventories.slots(container).len() as u32).find(|slot| {
            self.inventories
                .slot(container, *slot)
                .filter(|item| *item != 0)
                .and_then(|item| self.inventories.item(item))
                .is_some_and(|item| item.slot_data.item_uuid == item_uuid)
        }) else {
            debug!(item_uuid, message, "that item is not attached to that message");

            return Vec::new();
        };

        let Some(target) = self.inventories.first_free_rucksack_slot(self.player_entity_id) else {
            warn!("rucksack full; the attachment stays in the message");

            // Re-sync anyway, so the client gets its blanked square back.
            return self.sync_mailbox(world);
        };

        let effects =
            self.inventories
                .transfer_to_slot(container, source, self.player_entity_id, target, 0);

        let mut out = self.apply(effects, world);

        out.extend(self.sync_mailbox(world));

        out
    }

    /// A uuid for a message, derived rather than drawn at random.
    ///
    /// Keeps the session a function from values to values: a random uuid would make every
    /// compose produce different bytes and put the whole flow beyond exact assertion.
    fn next_uuid(&self) -> String {
        format!(
            "00000000-0000-4000-9000-{:012x}",
            self.player_entity_id as u64 * 1_000_000 + self.mailbox.len() as u64,
        )
    }

    /// One dig tick on a voxel. The block gives way once enough of them land.
    ///
    /// **The three crack stages the player sees are client-side.** It streams one packet per
    /// tick, every one identical, and the server counts them and decides. Breaking on the
    /// first would make every block give way three times too fast, which is the sort of
    /// difference that is invisible in a unit test and obvious in the game.
    fn dig(&mut self, chunk: [u32; 3], voxel: [u32; 3]) -> Vec<Vec<u8>> {
        let ticks = self.dig_damage.entry((chunk, voxel)).or_insert(0);

        *ticks += 1;

        if *ticks < DIG_TICKS_TO_BREAK {
            debug!(?chunk, ?voxel, ticks = *ticks, "dig tick");

            return Vec::new();
        }

        self.dig_damage.remove(&(chunk, voxel));

        self.voxel_edits
            .insert((chunk, voxel), PartialChunkEditsSync::AIR);

        debug!(?chunk, ?voxel, "dug through");

        vec![Self::chunk_edit(chunk, voxel, PartialChunkEditsSync::AIR)]
    }

    /// The item hash the player is holding, and the block it places, if it places one.
    fn held_block(&self, world: &World) -> Option<(u32, u8)> {
        let held = self.held_resource()?;

        // The hotbar carries a hash and the table is keyed by name, so this is a reverse
        // lookup: which placeable block's resource hashes to what the hand holds.
        let material = world.geodata.placeable_for_hash(held)?;

        Some((held, material))
    }

    /// Take one item of `hash` out of the rucksack. False when there is none.
    fn take_one(&mut self, hash: u32) -> bool {
        let Some(slot) = (0..self.inventory().len() as u32).find(|slot| {
            self.inventories
                .slot(self.player_entity_id, *slot)
                .filter(|item| *item != 0)
                .and_then(|item| self.inventories.name(item))
                == Some(hash)
        }) else {
            return false;
        };

        !self.inventories.destroy(self.player_entity_id, slot, 1).is_empty()
    }

    /// One `PartialChunkEditsSync` changing a single voxel.
    fn chunk_edit(chunk: [u32; 3], voxel: [u32; 3], material: u8) -> Vec<u8> {
        encode(|w| {
            PartialChunkEditsSync {
                chunk,
                edits: vec![ChunkEdit {
                    voxel_index: material,
                    voxels: vec![voxel],
                }],
            }
            .encode(w)
        })
    }

    /// Every voxel this session has changed, for anything that has to rebuild the terrain.
    pub fn voxel_edits(&self) -> &std::collections::HashMap<([u32; 3], [u32; 3]), u8> {
        &self.voxel_edits
    }

    /// The entity this player has a container open on, or 0.
    pub fn using_entity(&self) -> u32 {
        self.using_entity
    }

    /// Whether a container's lid is currently shut.
    pub fn has_been_opened(&self, container: u32) -> bool {
        self.closed_lids.contains(&container)
    }

    /// Sync the player's own body: the parameters that open a container and hold the rucksack.
    fn sync_player(&self, world: &World) -> Vec<Vec<u8>> {
        self.sync_of(self.player_entity_id, &["usingentityid"], world)
            .into_iter()
            .collect()
    }

    /// Sync a container: its lid, and what is in it.
    fn sync_container(&self, container: u32, world: &World) -> Vec<Vec<u8>> {
        self.sync_of(container, &["hasbeenopened"], world)
            .into_iter()
            .collect()
    }

    /// One `EntitySync` for `entity`, carrying only `parameters`.
    fn sync_of(&self, entity: u32, parameters: &[&str], world: &World) -> Option<Vec<u8>> {
        let (built, definition) = self.entity_now(entity, world)?;

        let sync_data = built.sync_data_for(definition, parameters);

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

    /// An entity as it is *now*, with this session's state written into it.
    ///
    /// The world's copy was built at startup. What has changed since -- what is in an
    /// inventory, whether a lid is shut, where a player's `usingentityid` points -- lives on
    /// the session, so re-encoding has to fold it back in.
    fn entity_now<'a>(
        &'a self,
        entity: u32,
        world: &'a World,
    ) -> Option<(Entity, &'a skysaga_world::EntityDefinition)> {
        if entity == self.player_entity_id {
            let (mut player, definition) =
                world.player_entity(&self.character, entity, self.inventory())?;

            for component in &mut player.components {
                match component {
                    Component::UseEntity(use_entity) => {
                        use_entity.using_entity_id = self.using_entity;
                    }

                    // The whole inbox is this one parameter.
                    Component::MailBox(mailbox) => {
                        mailbox.mail = self
                            .mailbox
                            .iter()
                            .map(|mail| skysaga_world::MailItem {
                                subject: mail.subject.clone(),
                                body: mail.body.clone(),
                                unknown: String::new(),
                                // Not a clock: the session is a pure function from values to
                                // values, and a real timestamp would make every sync differ.
                                // The client renders it as a date and nothing depends on it.
                                timestamp: 0,
                                message_uuid: mail.uuid.clone(),
                                attachment_entity: mail.attachment_entity,
                                text_arguments: Vec::new(),
                                flags: mail.flags,
                            })
                            .collect();
                    }

                    _ => {}
                }
            }

            return Some((player, definition));
        }

        let container = self.container(entity, world)?;

        let mut built = container.entity.clone();

        for component in &mut built.components {
            match component {
                Component::Inventory(inventory) => {
                    inventory.inventory_entity_list = self.inventories.slots(entity).to_vec();
                }

                Component::Interaction(interaction) => {
                    interaction.has_been_opened = self.closed_lids.contains(&entity);
                }

                _ => {}
            }
        }

        Some((built, &container.definition))
    }

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

            // Works for the player and for a container alike. A chest is an entity the client
            // is drawing too, so syncing only the player after a transfer leaves the chest's
            // square still showing an item that has moved.
            Effect::SlotsChanged { owner } => {
                let packet = self.sync_of(owner, &["inventoryentitylist"], world);

                if packet.is_none() {
                    warn!(owner, "no definition for this inventory; not syncing it");
                }

                packet
            }
        }
    }

    /// One item entity, ready to serialise.
    fn item_entity<'a>(
        &'a self,
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

/// One message in a player's inbox.
///
/// The whole inbox is a single entity parameter -- `Player.mailitemlist`, sync index 50 -- so
/// this is the server's own record, and [`Session::sync_mailbox`] is what turns it into the
/// list the client reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub uuid: String,
    pub subject: String,
    pub body: String,

    /// The container entity holding this message's attachments.
    ///
    /// An entity with an inventory, exactly as a chest is. Its slot layout is the awkward
    /// part -- see [`MAIL_ATTACHMENT_BASE`].
    pub attachment_entity: u32,

    /// The flag byte the client reads.
    ///
    /// | bit | meaning |
    /// |---:|---|
    /// | 0 | read |
    /// | 1 | unknown; nothing sets it |
    /// | 2 | this message offers a gift choice |
    /// | 3 | a gift has been chosen |
    pub flags: u8,
}

impl Mail {
    const READ: u8 = 1 << 0;
    const GIFT_CHOSEN: u8 = 1 << 3;

    pub fn is_read(&self) -> bool {
        self.flags & Self::READ != 0
    }

    pub fn gift_chosen(&self) -> bool {
        self.flags & Self::GIFT_CHOSEN != 0
    }

    pub fn attachment_entity(&self) -> u32 {
        self.attachment_entity
    }

    fn set_read(&mut self) {
        self.flags |= Self::READ;
    }

    fn set_gift_chosen(&mut self) {
        self.flags |= Self::GIFT_CHOSEN;
    }
}

/// A container that was spawned while the server was running.
#[derive(Debug, Clone)]
pub struct Spawned {
    pub entity: u32,

    /// Where it went, in the client's position units.
    pub position: [u32; 3],

    /// What to send so the client knows it is there, **in order**: the loot first, then the
    /// chest whose slot list names it.
    pub packets: Vec<Vec<u8>>,
}
