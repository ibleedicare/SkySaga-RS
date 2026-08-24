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
//! C->S ClientReadyToPlay          -> SetClientEntity, TimeSync, DebugRequestFinishTutorial
//! ```

pub mod combat;
pub mod server;
pub mod world;

pub use server::{GameServer, GameServerConfig};
pub use world::{Device, PlacedDevice, VoxelEdit, World, WorldConfig};

use std::collections::{BTreeMap, BTreeSet};

use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::customisation::CustomisationData;
use skysaga_proto::packets::chat::{RequestChatChannelData, SendChatChannelData};
use skysaga_proto::packets::combat::{
    EntityStoppedUsingEquippedItem, EntityUsedEquippedItem, EquippedItemUsed, EventEffect,
    IFellTooFar, KillOccurred, PerformEntityActions, PlayerDodged, PlayerFallenOffTheWorld,
    PlayerSpawned,
    RequestRespawn, SetPlayerState, StopUsingEquippedItem,
};
use skysaga_proto::packets::interaction::{Action, ExecuteEntityAction, InteractWithEntity};
use skysaga_proto::packets::mail::{
    DeleteMail, MailCheck, MailGiftSelected, MailRead, NewMailReceived, RemoteMailSynced,
    TakeMailAttachment,
};
use skysaga_proto::packets::movement::{EntityMoved, SetLookAtDirection, ANGLE_UNITS_PER_DEGREE};
use skysaga_proto::packets::crafting::{
    CollectCraftedItemInSlot, CraftingFailed, CraftingNotification, CraftingQueryQueue, ItemSpec,
    MoveItemToCraftingDropSlot, NewResourceEncountered, PerformCraftingDropSlotAction,
    QueueRecipeOnEntity, RemoveItemFromCraftingDropSlot,
};
use skysaga_proto::packets::todo_list::{
    TodoListTaskAdd, TodoListTaskRef, TodoTask, TASK_LIST_DEFAULT,
};
use skysaga_proto::packets::voxel::{ChunkEdit, PartialChunkEditsSync, PerformVoxelActions};
use skysaga_proto::packets::inventory::{
    InventoryItemDestroy, InventoryItemSwap, InventoryItemTransferAll, InventoryItemTransferToSlot,
    RequestEquipInventoryItem, RequestUiSettingsSetActiveSlot, RequestUiSettingsSlotChange,
    RequestUnEquipInventoryItem,
};
use skysaga_proto::packets::{
    BeginSync, EntityAdd, EntityRemoved, EntitySync, CharacterCreationResponse,
    ClientEntitiesSyncFinished, CreateHomeworld, DebugRequestFinishTutorial, NotifyPhotoCaptured,
    PhotoValidated, SaveCharacterName, SetCharacterCustomisationData, SetClientEntity,
    TimeSync, TransferToServer,
};
use skysaga_world::geodata::EquippedAction;
use skysaga_world::loot::Seeded;
use skysaga_world::inventory::{Effect, Inventories, StackLimits};
use skysaga_world::{
    Component, Entity, HealthComponent, ResourcePickupComponent, TransformComponent,
};
use tracing::{debug, info, warn};

/// What a hard landing costs, in hit points.
///
/// **Ours to choose.** `IFellTooFar` carries no height and the client's own reaction to it is
/// an empty function, so a fall means exactly what the server decides it means. One heart is
/// enough to be felt without making the island lethal, and ten of them is a death, which is
/// what makes the death-and-respawn loop reachable at all while nothing else can hurt a
/// player.
const FALL_DAMAGE: u32 = 4;

/// The entity a floor drop is, from `Entities.json`.
const PICKUP: &str = "Pickup";

/// The station a hand craft names, as a resource like any other.
///
/// The data models hand crafting as a station whose entity happens to be the player's own, so
/// this is a name from `geodata.json` and not a special case in the protocol.
const HAND_CRAFTING: &str = "Hand_Crafting";

/// How many crafting drop squares the panel has: one to repair with, one to dismantle in.
///
/// Two, and the client's `craftingdropslots` list has a minimum and a maximum of two, so its
/// count field is zero bits wide. See `CraftingDropSlotsComponent`.
const DROP_SLOTS: usize = 2;

/// The drop square that repairs, by index.
const REPAIR_SLOT: usize = 0;

/// The drop square that dismantles.
const DISMANTLE_SLOT: usize = 1;

/// How far ahead of the server a client's craft timer may run and still be honoured.
///
/// **The two never agree exactly, and they are not supposed to.** The server times a craft by
/// the recipe's `ExecutionTimeInSeconds`; the client divides by a duration `FUN_008ab190`
/// derives from the recipe *and the materials chosen*, and the clock it compares against is
/// its own, rebased onto ours by `TimeSync` across a network hop.
///
/// Observed against a live client: it asked to collect a three-second craft at
/// `elapsed_ms=2795`, was refused, and re-asked every thirty milliseconds until the server
/// came round -- seven wasted round trips for a craft that was, by the client's own reckoning,
/// finished. A quarter of a second of slack absorbs that. It is far too little to be worth
/// anything to someone shortening a craft on purpose, and it is the difference between one
/// exchange and eight.
const COLLECT_GRACE_MS: u64 = 250;

/// Whether to send `CraftingNotification` (67), the "your item is ready" toast.
///
/// # A diagnostic lever, and a warning about how one is used
///
/// This exists because a live client hung with a craft on its panel, and the two toasts were
/// the only bytes on that wire a working server had never sent. Suppressing both appeared to
/// fix it and the packet was blamed here in writing -- wrongly. The hang was reproducible only
/// when the craft was started **without dragging the materials into the recipe's slot**: the
/// client sends `QueueRecipeOnEntity` from the button anyway, and then waits for ever on a
/// craft its own panel never considers begun. Driven properly -- pick the recipe, drag the
/// resource into the square, press craft -- four consecutive crafts queue, run and collect with
/// both toasts sent.
///
/// The lesson is worth more than the switch: a bisect over server packets is meaningless while
/// the *client* is being driven differently between runs.
///
/// `SKYSAGA_CRAFT_NOTIFICATION=0` suppresses it.
fn craft_notification_enabled() -> bool {
    std::env::var("SKYSAGA_CRAFT_NOTIFICATION").as_deref() != Ok("0")
}

/// Whether tools and armour are minted as `DurableInventoryItem`.
///
/// **Off by default, and the reason is the wire format rather than the feature.** The widths of
/// `durability` and `durabilitymax` are not known: no capture carries one and the C# oracle
/// never wrote one. They are sync indices 1 and 2 against `inventoryslotdata`'s 5, so a wrong
/// width shifts the slot data and every rucksack square draws wrong. Turn it on with
/// `SKYSAGA_DURABLE_ITEMS=1` to sweep the width in front of a client; see
/// `skysaga_world::components::durability`.
fn durable_items_enabled() -> bool {
    match DURABLE_ITEMS.load(std::sync::atomic::Ordering::Relaxed) {
        UNSET => {
            let from_env = std::env::var("SKYSAGA_DURABLE_ITEMS").as_deref() == Ok("1");

            set_durable_items(from_env);

            from_env
        }

        state => state == ON,
    }
}

/// Turn durable items on or off while the server is running.
///
/// Runtime rather than start-up, because the point is to *sweep*: the width has to be tried,
/// looked at in the client, and tried again, and a restart between each costs a minute of
/// loading screen. Pairs with `skysaga_world::components::durability::set_bits`.
pub fn set_durable_items(on: bool) {
    DURABLE_ITEMS.store(if on { ON } else { OFF }, std::sync::atomic::Ordering::Relaxed);
}

/// Three states, because "not asked yet" has to be told apart from "asked, and off": the
/// environment is read once, on the first question, and a later call may still override it.
const UNSET: u8 = 0;
const ON: u8 = 1;
const OFF: u8 = 2;

static DURABLE_ITEMS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(UNSET);

/// Whether to send `NewResourceEncountered` (43), the discovery popup. See above.
fn discovery_toasts_enabled() -> bool {
    std::env::var("SKYSAGA_DISCOVERY_TOASTS").as_deref() != Ok("0")
}

/// A player's full health: `PhysicalProperties > player > Durability > Player > Health`.
const PLAYER_HEALTH: u32 = 40;

/// Reach for an attacker whose properties cannot be resolved, in voxels.
///
/// `Reaches > Default`. Reached only when the data file is missing, where every other number
/// is a default too.
const DEFAULT_REACH: f32 = 2.0;

/// The `EventEffect` type played on a hit.
///
/// **Unconfirmed.** Two of the 0x61 effect ids are known -- 0x2E is block and 0x21 is spawn --
/// and neither is the hit spark. The client's dispatch has recovered cases at 4, 0x1c, 0x27,
/// 0x2a, 0x2b, 0x2e and 0x3d, and 4 is the lowest of them; an id with no case falls to a
/// default that plays a generic effect, so a wrong guess is a wrong spark and never a stall.
const HIT_EFFECT: u32 = 4;

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

/// How many distinct payloads of one unhandled id are worth reporting.
///
/// Enough to read a small field's range off the log, few enough that a packet carrying a
/// timestamp cannot flood it.
pub const UNHANDLED_SAMPLES: usize = 8;

/// How much of an unhandled packet is kept for the log.
///
/// Enough for a layout to be read off -- the client's packets are small -- and short enough
/// that one line stays one line.
pub const UNKNOWN_PAYLOAD_BYTES: usize = 24;

/// How many dig ticks break a block.
///
/// From the C#. The client streams one packet per tick and every field is identical across
/// the run, so the count is the server's to keep.
pub const DIG_TICKS_TO_BREAK: u32 = 3;

/// Why a request for an item was not granted.
///
/// The two are worth telling apart in what the player is told: one is a name the game has never
/// heard of, and the other is a rucksack they filled themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveRefusal {
    /// No `Resources` entry has this name. See [`Session::give_checked`].
    UnknownItem,

    /// Every rucksack square is taken.
    RucksackFull,
}

impl std::fmt::Display for GiveRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownItem => write!(f, "no such item"),
            Self::RucksackFull => write!(f, "the rucksack is full"),
        }
    }
}

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
    RequestUnEquipInventoryItem(RequestUnEquipInventoryItem),

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

    // --- crafting ------------------------------------------------------------------------
    //
    // A station craft names the station's entity; a hand craft names the player's own. The
    // packets are identical either way, which is why the station is validated against the
    // recipe rather than read off a flag.
    /// 39, make this, here.
    QueueRecipeOnEntity(QueueRecipeOnEntity),

    /// 40, take what a finished slot holds.
    CollectCraftedItemInSlot(CollectCraftedItemInSlot),

    /// 41, "what is in this station's queue?". Answered with a sync, not a reply packet.
    CraftingQueryQueue(CraftingQueryQueue),

    // --- the drop slots ------------------------------------------------------------------
    //
    // The pocket-crafting panel's repair and dismantle tabs. `slotType` is an index into the
    // two-element drop-slot array and not a verb: 0 is repair, 1 is dismantle.
    /// 153, an item was dragged onto a drop square.
    MoveItemToCraftingDropSlot(MoveItemToCraftingDropSlot),

    /// 154, and dragged back out.
    RemoveItemFromCraftingDropSlot(RemoveItemFromCraftingDropSlot),

    /// 155, the repair or dismantle button was pressed.
    PerformCraftingDropSlotAction(PerformCraftingDropSlotAction),

    // --- the quest log -------------------------------------------------------------------
    //
    // All four are answered the same way: mutate `tasklist` and sync it back. There is no
    // reply packet -- the component sync *is* the reply, as it is for the wallet.
    /// 146, put an objective on the list.
    TodoListTaskAdd(TodoListTaskAdd),

    /// 145, delete a task for good.
    TodoListTaskErase(TodoListTaskRef),

    /// 147, take a task off the visible list, keeping it.
    TodoListTaskRemove(TodoListTaskRef),

    /// 148, put a removed task back.
    TodoListTaskReAdd(TodoListTaskRef),

    // --- combat --------------------------------------------------------------------------
    //
    // The whole client-to-server half of a fight. There is no hit packet: what arrives is
    // "the button went down", and the server decides everything that follows.
    /// 194 — the swing. Carries the CRC of a GeoData action, which is the damage table.
    EquippedItemUsed(EquippedItemUsed),

    /// 152 — **the hit**. The client's own detection, naming the entity it struck.
    PerformEntityActions(PerformEntityActions),

    /// 195 — the button came up. Echoed so other players' animations end.
    StopUsingEquippedItem(StopUsingEquippedItem),

    /// 169 — "I am attacking / dodging / dead". Relayed by the server layer as bytes.
    SetPlayerState(SetPlayerState),

    /// 292 — a roll. Decoded so it stops reading as an unhandled packet.
    PlayerDodged(PlayerDodged),

    /// 158 — "I landed hard". Zero fields, and the client's own reaction is a no-op: what a
    /// fall costs is entirely the server's choice.
    IFellTooFar,

    /// 290 — "I am below the world". Zero fields, sent **once** and latched. Answering with
    /// nothing leaves the player frozen forever.
    PlayerFallenOffTheWorld,

    /// 221 — the respawn button on the death screen. Zero fields.
    RequestRespawn,

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
    /// A packet this server does not handle, with the bytes it arrived as.
    ///
    /// The payload is kept because an unhandled id is a reversing lead and the id alone is not
    /// enough: the layout is reconstructed from what the client actually sent. Truncated to
    /// [`UNKNOWN_PAYLOAD_BYTES`], since this exists to be read in a log line.
    Unknown { wire_id: u16, payload: Vec<u8> },
}

impl ClientPacket {
    /// Classify a whole packet, body included.
    ///
    /// Dispatching on the id alone is not enough: `SaveCharacterName` *is* its body, and
    /// losing it means answering the client without knowing what it asked.
    ///
    /// A body that fails to decode falls back to `Unknown` rather than panicking -- these are
    /// bytes from an untrusted peer.
    /// An unhandled packet, keeping the first [`UNKNOWN_PAYLOAD_BYTES`] of it.
    fn unknown(wire_id: u16, bytes: &[u8]) -> Self {
        Self::Unknown {
            wire_id,
            payload: bytes.iter().take(UNKNOWN_PAYLOAD_BYTES).copied().collect(),
        }
    }

    pub fn parse(bytes: &[u8]) -> Self {
        let mut reader = BitReader::from_bytes(bytes);

        let Ok(id) = reader.read_packet_id() else {
            return Self::unknown(0, bytes);
        };

        let wire_id = id + ID_USER_PACKET_ENUM;

        match id {
            SaveCharacterName::ID => SaveCharacterName::decode(&mut reader)
                .map(Self::SaveCharacterName)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            CreateHomeworld::ID => CreateHomeworld::decode(&mut reader)
                .map(Self::CreateHomeworld)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            SetCharacterCustomisationData::ID => SetCharacterCustomisationData::decode(&mut reader)
                .map(Self::SetCharacterCustomisation)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            NotifyPhotoCaptured::ID => NotifyPhotoCaptured::decode(&mut reader)
                .map(Self::NotifyPhotoCaptured)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            InventoryItemTransferToSlot::ID => InventoryItemTransferToSlot::decode(&mut reader)
                .map(Self::InventoryItemTransferToSlot)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            InventoryItemSwap::ID => InventoryItemSwap::decode(&mut reader)
                .map(Self::InventoryItemSwap)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            InventoryItemTransferAll::ID => InventoryItemTransferAll::decode(&mut reader)
                .map(Self::InventoryItemTransferAll)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            InventoryItemDestroy::ID => InventoryItemDestroy::decode(&mut reader)
                .map(Self::InventoryItemDestroy)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RequestUnEquipInventoryItem::ID => RequestUnEquipInventoryItem::decode(&mut reader)
                .map(Self::RequestUnEquipInventoryItem)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RequestEquipInventoryItem::ID => RequestEquipInventoryItem::decode(&mut reader)
                .map(Self::RequestEquipInventoryItem)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RequestUiSettingsSlotChange::ID => RequestUiSettingsSlotChange::decode(&mut reader)
                .map(Self::RequestUiSettingsSlotChange)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RequestUiSettingsSetActiveSlot::ID => {
                RequestUiSettingsSetActiveSlot::decode(&mut reader)
                    .map(Self::RequestUiSettingsSetActiveSlot)
                    .unwrap_or(Self::unknown(wire_id, bytes))
            }

            EntityMoved::ID => EntityMoved::decode(&mut reader)
                .map(Self::EntityMoved)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            SetLookAtDirection::ID => SetLookAtDirection::decode(&mut reader)
                .map(Self::SetLookAtDirection)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            ExecuteEntityAction::ID => ExecuteEntityAction::decode(&mut reader)
                .map(Self::ExecuteEntityAction)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            InteractWithEntity::ID => InteractWithEntity::decode(&mut reader)
                .map(Self::InteractWithEntity)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            PerformVoxelActions::ID => PerformVoxelActions::decode(&mut reader)
                .map(Self::PerformVoxelActions)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            QueueRecipeOnEntity::ID => QueueRecipeOnEntity::decode(&mut reader)
                .map(Self::QueueRecipeOnEntity)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            CollectCraftedItemInSlot::ID => CollectCraftedItemInSlot::decode(&mut reader)
                .map(Self::CollectCraftedItemInSlot)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            CraftingQueryQueue::ID => CraftingQueryQueue::decode(&mut reader)
                .map(Self::CraftingQueryQueue)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            MoveItemToCraftingDropSlot::ID => MoveItemToCraftingDropSlot::decode(&mut reader)
                .map(Self::MoveItemToCraftingDropSlot)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RemoveItemFromCraftingDropSlot::ID => {
                RemoveItemFromCraftingDropSlot::decode(&mut reader)
                    .map(Self::RemoveItemFromCraftingDropSlot)
                    .unwrap_or(Self::unknown(wire_id, bytes))
            }

            PerformCraftingDropSlotAction::ID => PerformCraftingDropSlotAction::decode(&mut reader)
                .map(Self::PerformCraftingDropSlotAction)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            TodoListTaskAdd::ID => TodoListTaskAdd::decode(&mut reader)
                .map(Self::TodoListTaskAdd)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            TodoListTaskRef::ERASE => TodoListTaskRef::decode(&mut reader)
                .map(Self::TodoListTaskErase)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            TodoListTaskRef::REMOVE => TodoListTaskRef::decode(&mut reader)
                .map(Self::TodoListTaskRemove)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            TodoListTaskRef::READD => TodoListTaskRef::decode(&mut reader)
                .map(Self::TodoListTaskReAdd)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            EquippedItemUsed::ID => EquippedItemUsed::decode(&mut reader)
                .map(Self::EquippedItemUsed)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            PerformEntityActions::ID => PerformEntityActions::decode(&mut reader)
                .map(Self::PerformEntityActions)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            StopUsingEquippedItem::ID => StopUsingEquippedItem::decode(&mut reader)
                .map(Self::StopUsingEquippedItem)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            SetPlayerState::ID => SetPlayerState::decode(&mut reader)
                .map(Self::SetPlayerState)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            PlayerDodged::ID => PlayerDodged::decode(&mut reader)
                .map(Self::PlayerDodged)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            MailCheck::ID => MailCheck::decode(&mut reader)
                .map(Self::MailCheck)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            MailRead::ID => MailRead::decode(&mut reader)
                .map(Self::MailRead)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            MailGiftSelected::ID => MailGiftSelected::decode(&mut reader)
                .map(Self::MailGiftSelected)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            DeleteMail::ID => DeleteMail::decode(&mut reader)
                .map(Self::DeleteMail)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            RequestChatChannelData::ID => RequestChatChannelData::decode(&mut reader)
                .map(Self::RequestChatChannelData)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            TakeMailAttachment::ID => TakeMailAttachment::decode(&mut reader)
                .map(Self::TakeMailAttachment)
                .unwrap_or(Self::unknown(wire_id, bytes)),

            _ => Self::from_wire_id_with(wire_id, bytes),
        }
    }

    /// Classify by *wire* id alone, for the body-less handshake packets.
    ///
    /// The payload-carrying form is [`Self::from_wire_id_with`]; this one is for callers that
    /// have an id and no bytes, which is every test that names a handshake step.
    pub fn from_wire_id(wire_id: u16) -> Self {
        Self::from_wire_id_with(wire_id, &[])
    }

    /// The same, keeping the bytes when the packet turns out to be one nothing handles.
    pub fn from_wire_id_with(wire_id: u16, bytes: &[u8]) -> Self {
        match wire_id {
            135 => Self::ClientConnected,
            136 => Self::ClientReadyToSync,
            137 => Self::ClientReadyToPlay,
            138 => Self::ClientInitialSyncFinished,

            // The body-less combat packets. Their arrival is the whole message.
            id if id == IFellTooFar::ID + ID_USER_PACKET_ENUM => Self::IFellTooFar,
            id if id == PlayerFallenOffTheWorld::ID + ID_USER_PACKET_ENUM => {
                Self::PlayerFallenOffTheWorld
            }
            id if id == RequestRespawn::ID + ID_USER_PACKET_ENUM => Self::RequestRespawn,

            other => Self::unknown(other, bytes),
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
    facing_yaw: Option<i32>,

    /// The entity whose container this player has open, or 0 for none.
    ///
    /// **This is the whole opening mechanism.** There is no "open the container" packet: the
    /// client opens a loot window when the *player's* `usingentityid` becomes the target's id,
    /// and closes it when that goes back to 0.
    using_entity: u32,

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

    /// Whether a close raises `hasbeenopened`. See [`Session::set_raise_lid_on_close`].
    raise_lid_on_close: bool,

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

    /// Hit points taken off this player.
    player_damage: u32,

    /// What each equip slot is currently swinging, by `location`.
    ///
    /// **The join between the two halves of a hit.** `EquippedItemUsed` names the action but
    /// not the target; `PerformEntityActions` names the target but not the action. The slot id
    /// is the only field they share, so this is what makes a landed blow worth anything.
    ///
    /// Set on the swing and cleared on the release, so a hit arriving from a slot that is not
    /// swinging is dropped rather than credited to whatever was used last.
    armed: std::collections::HashMap<u32, EquippedAction>,


    /// What is queued or waiting to be collected, per station, one entry per occupied slot.
    ///
    /// **Keyed by the station's entity, and the player is one of them.** Hand crafting is not a
    /// special case: the player carries a crafting component with one slot, an `Anvil` carries
    /// one with three, and the packets name whichever entity the panel was opened on. One
    /// shared queue would have a craft started at the anvil appear in the player's own hands.
    ///
    /// On the session rather than the world because every station a player can reach is one
    /// they placed, and placements are per session too. Both move together.
    crafting: BTreeMap<u32, Vec<skysaga_world::CraftingSlot>>,

    /// What is sitting on the two crafting drop squares, by item entity id, 0 for empty.
    ///
    /// **Square 0 repairs and square 1 dismantles**, which the client decides and the server
    /// only has to agree with: `FUN_008d72a0` refuses a drop into 0 for an item with no
    /// durability, and `FUN_007fd290` refuses one into 1 for a stack smaller than the recipe's
    /// output. Both refusals happen before anything is sent, so a server never sees them.
    ///
    /// The item is **out of the rucksack while it sits here**, exactly as the client draws it:
    /// the inventory square empties and the drop square fills.
    drop_slots: [u32; DROP_SLOTS],

    /// Whether this session has been handed back what its account was carrying.
    ///
    /// A restore is refused once the rucksack holds anything, but this is what stops the
    /// server *asking* every tick, and what keeps the first tick from recording an empty
    /// rucksack over a stored one.
    items_restored: bool,

    /// The rucksack as last written down, so an unchanged tick costs nothing.
    recorded_items: Option<Vec<skysaga_state::StoredItem>>,

    /// Resources this player has been seen holding, so a discovery is announced once.
    ///
    /// The client keeps a set of its own -- `FUN_00878100` -- so a repeat costs only a packet;
    /// this is what stops the toast firing every time a stack changes size.
    seen_resources: BTreeSet<u32>,

    /// A fixed wall clock, in milliseconds since the Unix epoch, or `None` for the real one.
    ///
    /// Crafting is the first thing here that needs the time of day rather than a tick count:
    /// a slot carries the moment its craft *started* and the client divides by the recipe's
    /// duration to get a progress bar. Reading `SystemTime::now()` inline would make every
    /// crafting test depend on how fast it runs, so the clock is a value the tests can pin.
    clock_ms: Option<u64>,

    /// The quest log, one entry per row the client shows.
    ///
    /// On the session for the same reason as `crafting`: it lives on the player's own entity.
    /// The client drives it entirely, the server allocates ids and stores what it is told , 
    /// because nothing yet activates a job challenge, which is what would auto-add a row.
    todo_tasks: Vec<TodoTask>,

    /// Where loot rolls come from.
    ///
    /// Seeded from the player's entity id rather than a clock, so this crate keeps its
    /// no-I/O rule and a session's drops are reproducible. Two players rolling the same
    /// table get different results because their bodies have different ids.
    loot_rolls: Seeded,

    /// Packets for the *other* connections, drained by the server layer.
    ///
    /// The swing echo is the first thing a session produces that is not addressed to the
    /// client that caused it: `EntityUsedEquippedItem` is what makes another player's sword
    /// move, and the swinger's own client discards it.
    broadcasts: Vec<Vec<u8>>,

    /// Unhandled packets already reported, as `(wire id, payload)`.
    ///
    /// EntityMoved alone arrives dozens of times a minute, so warning per packet buries every
    /// other line. But the payload is the reason to look at all: an id names a gap and the
    /// bytes are what a layout is reconstructed from, and one sample is not a layout. So a
    /// repeat of the *same* bytes is silent, a new payload for a known id is not, and after
    /// [`UNHANDLED_SAMPLES`] distinct ones that id goes quiet for good -- a field that moves
    /// every tick would otherwise report every tick.
    reported: BTreeSet<(u16, Vec<u8>)>,
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
            raise_lid_on_close: true,
            closed_lids: BTreeSet::new(),
            dig_damage: std::collections::HashMap::new(),
            spawned: Vec::new(),
            player_damage: 0,
            armed: std::collections::HashMap::new(),
            crafting: BTreeMap::new(),
            drop_slots: [0; DROP_SLOTS],
            seen_resources: BTreeSet::new(),
            items_restored: false,
            recorded_items: None,
            clock_ms: None,
            todo_tasks: Vec::new(),
            loot_rolls: Seeded::new(u64::from(player_entity_id)),
            broadcasts: Vec::new(),
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
    /// The inventory model, for the few callers that change it directly.
    ///
    /// Public for tests and for the admin path; everything a *client* can do goes through a
    /// packet handler instead, so the model and the packets cannot drift apart.
    pub fn inventories_mut(&mut self) -> &mut Inventories {
        &mut self.inventories
    }

    /// Empty every rucksack square, leaving equipment alone.
    ///
    /// **So a driven test can control its own preconditions.** An admin give takes the first
    /// *free* square, which depends on everything the player did before; after this the next
    /// give lands in the first square, every time. Equipment is squares 0 to 8 and is not
    /// touched: a test that wanted an empty bag did not ask to be undressed.
    pub fn clear_rucksack(&mut self) -> Vec<Effect> {
        let owner = self.player_entity_id;

        let held: Vec<u32> = self
            .inventory()
            .iter()
            .enumerate()
            .skip(skysaga_world::inventory::FIRST_RUCKSACK_SLOT as usize)
            .filter(|(_, item)| **item != 0)
            .map(|(slot, _)| slot as u32)
            .collect();

        let mut effects = Vec::new();

        for slot in held {
            if let Some((_, removed)) = self.inventories.detach(owner, slot) {
                effects.extend(removed);
            }
        }

        if !effects.is_empty() {
            effects.push(Effect::SlotsChanged { owner });
        }

        effects
    }

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
    pub fn facing_yaw(&self) -> Option<i32> {
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

    /// Create a stack of `item`, refusing a name the game does not define.
    ///
    /// [`Self::give`] hashes whatever it is handed, and the hash of a misspelling is a
    /// perfectly good number that resolves to no resource: the stack is minted, every
    /// server-side signal reports success, and the client draws an empty square. This is the
    /// same thing with the one check that makes that impossible, and it is what the admin
    /// commands go through. `give` stays unchecked for seeding and for tests, which name items
    /// as literals.
    pub fn give_checked(
        &mut self,
        item: &str,
        count: u32,
        world: &World,
    ) -> Result<u32, GiveRefusal> {
        if !world.geodata.knows_resource(item) {
            return Err(GiveRefusal::UnknownItem);
        }

        self.give(item, count).ok_or(GiveRefusal::RucksackFull)
    }

    /// Whether this session has already been handed back what it was carrying.
    pub fn items_restored(&self) -> bool {
        self.items_restored
    }

    /// Say that it has, so a later tick does not do it again.
    pub fn mark_items_restored(&mut self) {
        self.items_restored = true;
    }

    /// What to write down, or `None` when nothing has changed since the last time.
    ///
    /// The comparison lives here so the game loop can ask every tick: a tick where nobody moved
    /// an item costs one walk of the rucksack and no lock, where calling into `AppState` would
    /// take a write lock to discover the same thing.
    pub fn items_to_record(&mut self) -> Option<Vec<skysaga_state::StoredItem>> {
        // Nothing is worth recording until the restore has happened. Otherwise the first tick
        // of a session writes down an empty rucksack and erases what the player had.
        if !self.items_restored {
            return None;
        }

        let items = self.carried_items();

        if self.recorded_items.as_ref() == Some(&items) {
            return None;
        }

        self.recorded_items = Some(items.clone());

        Some(items)
    }

    /// What this player is carrying, square by square, for writing down.
    ///
    /// Only the occupied squares: an empty rucksack is no rows rather than 45 empty ones. The
    /// item is its name hash, because a stack's *entity* is minted per session and means
    /// nothing tomorrow. See [`skysaga_state::StoredItem`].
    pub fn carried_items(&self) -> Vec<skysaga_state::StoredItem> {
        self.inventory()
            .iter()
            .enumerate()
            .filter(|(_, entity)| **entity != 0)
            .filter_map(|(slot, entity)| {
                let stack = self.inventories.item(*entity)?;

                Some(skysaga_state::StoredItem {
                    slot: slot as u32,
                    item: stack.slot_data.name?,
                    count: stack.slot_data.count,
                })
            })
            .collect()
    }

    /// Put a stored rucksack back, and say what the client must be told.
    ///
    /// **Each stack is announced before the slot list that names it.** The handshake burst
    /// carries no item entities at all, so a restore has to create them the way `/give` does:
    /// an `EntityAdd` per stack, then one sync of `inventoryentitylist`. A list naming an
    /// entity the client has not been told about draws an empty square.
    ///
    /// Does nothing when the player is already carrying something. The join path cannot be sure
    /// it runs exactly once, and a second restore would double a player's belongings.
    pub fn restore_items(
        &mut self,
        items: &[skysaga_state::StoredItem],
        world: &World,
    ) -> Vec<Vec<u8>> {
        if items.is_empty() || !self.carried_items().is_empty() {
            return Vec::new();
        }

        let mut effects = Vec::new();

        for item in items {
            let Some(entity) = self
                .inventories
                .give(self.player_entity_id, item.slot, item.item, item.count)
            else {
                warn!(slot = item.slot, "no such square to restore into");

                continue;
            };

            self.inventories.reserve_ids_from(self.inventories.next_entity_id());

            effects.push(Effect::ItemCreated { entity });
        }

        if effects.is_empty() {
            return Vec::new();
        }

        effects.push(Effect::SlotsChanged {
            owner: self.player_entity_id,
        });

        info!(squares = items.len(), "restored what the player was carrying");

        self.apply(effects, world)
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

    /// The unhandled packets reported so far, as `(wire id, payload)` in ascending order.
    pub fn reported_unhandled(&self) -> Vec<(u16, Vec<u8>)> {
        self.reported.iter().cloned().collect()
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

                // **The owner's name is this player's, not the world's placeholder.** The
                // client formats the island banner as `%S's %S` from this field, so a world
                // built once at startup would announce every island as belonging to whoever
                // the config named. The C# builds ServerInfo per connection for the same
                // reason.
                let mut info = world.server_info.clone();

                if let Some(owner) = self
                    .character
                    .name
                    .clone()
                    .or_else(|| self.account.clone())
                    .filter(|name| !name.is_empty())
                {
                    info.owner_name = owner;
                }

                vec![encode(|w| info.encode(w)), encode(|w| world.map.encode(w))]
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

                // Anything players have put down since the island was built, and anything that
                // has wandered in. Part of the world rather than of the burst's frozen entity
                // list, so a joiner is told about them here or never told at all.
                let since: Vec<u32> = world
                    .devices()
                    .iter()
                    .map(|device| device.id)
                    .chain(world.spawned_creatures().iter().map(|creature| creature.id))
                    .filter(|id| !world.is_dead(*id))
                    .collect();

                for id in since {
                    if let Some((built, definition)) = self.entity_now(id, world) {
                        out.push(encode(|w| built.to_entity_add(definition).encode(w)));
                    }
                }

                // ...and anything lying on the floor. Two entities each, **the stack before the
                // pickup that names it**, and the stack has to exist in this session's own model
                // before it can be encoded: it was minted in whichever session dropped it.
                for drop in world.floor_drops() {
                    self.ensure_stack(&drop);

                    if let Some(stack) = self.inventories.item(drop.stack).cloned() {
                        if let Some(definition) = world.item_definition() {
                            let entity = Entity::new(
                                drop.stack,
                                vec![Component::InventoryItem(stack)],
                            );

                            out.push(encode(|w| entity.to_entity_add(definition).encode(w)));
                        }
                    }

                    if let Some(definition) = world.definitions.get(PICKUP) {
                        let entity = Entity::new(
                            drop.pickup,
                            world::pickup_components(drop.stack, drop.position),
                        );

                        out.push(encode(|w| entity.to_entity_add(definition).encode(w)));
                    }
                }

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
                    // **Give the client a clock.** Until this arrives its `now()` counts
                    // milliseconds since the client launched, not since the epoch, so every
                    // real timestamp the server sends is far in its future. A crafting slot's
                    // start time is one of those, and the craft then sits at 0% for ever.
                    // Nothing else sets it: the client's clock globals have exactly one writer
                    // reachable from the network, and this is the packet that reaches it.
                    encode(|w| {
                        TimeSync {
                            now_ms: self.now_ms(),
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

            (ClientPacket::RequestUnEquipInventoryItem(packet), _) => {
                // **The hands hold what the hotbar names**, so emptying one is not a move: the
                // model reports no effects and what has to change is the binding. Anything else
                // goes back into the first free rucksack square.
                if packet.equip_slot < 2 {
                    debug!(slot = packet.equip_slot, "emptied a hand");

                    self.hotbar.remove(&self.active_slot);

                    return self
                        .sync_of(self.player_entity_id, &["hotbarslotresources"], world)
                        .into_iter()
                        .collect();
                }

                let effects = self.inventories.unequip(self.player_entity_id, packet.equip_slot);

                debug!(
                    slot = packet.equip_slot,
                    moved = !effects.is_empty(),
                    "took something off",
                );

                self.apply(effects, world)
            }

            (ClientPacket::RequestUiSettingsSlotChange(packet), _) => {
                // **This packet numbers the squares from one and `SetActiveSlot` numbers them
                // from zero.** Measured against the retail client: the "1" key reports an
                // active slot of 0 and the "5" key reports 4, while dragging an item into the
                // fifth square reports a bind of 5. The client sends both for one action, a
                // tenth of a millisecond apart, so keying the hotbar by the raw numbers files
                // the item under 5 and then looks it up under 4.
                //
                // Everything downstream is kept in the zero-based numbering, because that is
                // the one the player's `activeslot` parameter uses.
                let square = packet.slot.saturating_sub(1);

                debug!(
                    slot = packet.slot,
                    square,
                    resource = packet.resource,
                    "hotbar bound",
                );

                self.hotbar.insert(square, packet.resource);

                // A fresh bind is also what the player just selected: the client does not
                // always follow one with a SetActiveSlot.
                self.active_slot = square;

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

                match packet.action {
                    // Only Interact opens anything. Opening on any action at all would have a
                    // pickaxe swing open the loot window.
                    Some(Action::Interact) => self.open_container(packet.target_entity, world),

                    // Walking over a floor drop. Fired repeatedly while the player stands on
                    // it, so this has to be safe to answer many times.
                    Some(Action::ResourcePickup) => {
                        self.collect_pickup(packet.target_entity, world)
                    }

                    _ => Vec::new(),
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

            // --- crafting -----------------------------------------------------------------
            (ClientPacket::QueueRecipeOnEntity(packet), _) => self.queue_recipe(packet, world),

            (ClientPacket::CollectCraftedItemInSlot(packet), _) => {
                self.collect_craft(packet, world)
            }

            (ClientPacket::CraftingQueryQueue(packet), _) => {
                // No reply packet exists. The answer is the station's own queue, synced.
                self.sync_of(packet.entity_id, &["craftingslots"], world)
                    .into_iter()
                    .collect()
            }

            (ClientPacket::MoveItemToCraftingDropSlot(packet), _) => {
                self.move_to_drop_slot(packet, world)
            }

            (ClientPacket::RemoveItemFromCraftingDropSlot(packet), _) => {
                self.take_from_drop_slot(packet.slot_type, world)
            }

            (ClientPacket::PerformCraftingDropSlotAction(packet), _) => {
                self.perform_drop_slot_action(packet.slot_type, world)
            }

            // --- the quest log ------------------------------------------------------------
            (ClientPacket::TodoListTaskAdd(packet), _) => self.todo_add(packet, world),

            (ClientPacket::TodoListTaskErase(packet), _) => self.todo_erase(packet, world),

            // Remove and ReAdd are a visibility flip rather than a deletion. Nothing in the
            // record has been identified as a "hidden" bit, so until one is, both are
            // acknowledged with a sync and change nothing, which keeps the client's list and
            // the server's identical instead of silently diverging.
            (ClientPacket::TodoListTaskRemove(packet), _)
            | (ClientPacket::TodoListTaskReAdd(packet), _) => {
                debug!(task = packet.task_id, "todo visibility change");

                self.sync_of(self.player_entity_id, &["tasklist"], world)
                    .into_iter()
                    .collect()
            }

            // --- combat -------------------------------------------------------------------
            (ClientPacket::EquippedItemUsed(packet), _) => self.equipped_item_used(packet, world),

            (ClientPacket::PerformEntityActions(packet), _) => {
                self.perform_entity_action(packet, world)
            }

            (ClientPacket::StopUsingEquippedItem(packet), _) => {
                // The slot is no longer swinging, so a hit arriving after this is not worth
                // the action that was armed.
                self.armed.remove(&packet.location);

                // Nothing back to the swinger -- their own client already ended the animation.
                // The other players' clients did not, and this is what tells them.
                self.broadcasts.push(encode(|w| {
                    EntityStoppedUsingEquippedItem {
                        entity_id: self.player_entity_id,
                        location: packet.location,
                    }
                    .encode(w)
                }));

                Vec::new()
            }

            (ClientPacket::SetPlayerState(packet), _) => {
                // Relayed as raw bytes by the server layer, which is why nothing is built
                // here. Decoded so the state is in the log rather than an unhandled id.
                debug!(state = packet.state_id, "player state");

                Vec::new()
            }

            (ClientPacket::PlayerDodged(packet), _) => {
                // No immunity window yet: nothing attacks the player, so there is nothing for
                // one to protect against. Named rather than left unhandled because it arrives
                // paired with SetPlayerState on every roll.
                debug!(direction = packet.direction, "dodge");

                Vec::new()
            }

            (ClientPacket::IFellTooFar, _) => {
                debug!("a hard landing");

                self.hurt_player(FALL_DAMAGE, world)
            }

            (ClientPacket::PlayerFallenOffTheWorld, _) => {
                // **Answer or the client freezes.** It has already ragdolled itself and it
                // latches this packet, so it will never ask again. Respawning outright rather
                // than killing: falling through the world is the server's geometry failing,
                // not the player's mistake.
                warn!("a player fell out of the world");

                self.respawn(world)
            }

            (ClientPacket::RequestRespawn, _) => self.respawn(world),

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

            (ClientPacket::Unknown { wire_id, payload }, _) => {
                // An unimplemented packet is the usual reason a client stalls, so it is worth
                // seeing -- but only once per id. Ordinal is what the documentation tables are
                // keyed by, and the bytes are what a layout is reconstructed from: an id on its
                // own says a gap exists, the payload says what is in it.
                let samples = self
                    .reported
                    .iter()
                    .filter(|(seen, _)| *seen == wire_id)
                    .count();

                if samples < UNHANDLED_SAMPLES && self.reported.insert((wire_id, payload.clone()))
                {
                    warn!(
                        wire_id,
                        ordinal = wire_id.saturating_sub(ID_USER_PACKET_ENUM),
                        bytes = %payload.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
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
    /// The facing comes from `EntityMoved` in units of 1/32 of a degree: `FUN_007a46f0` builds
    /// it as `(int)(facingYaw * 32.0)` and `FUN_007a6010` writes it over `-12800..12800`. The
    /// C# reads that field as a float and gets a denormal, so its own chests always land due
    /// north whatever the player is doing.
    ///
    /// This used to take the field's declared maximum, 25600, as a full turn. That is the
    /// *width* rather than a circle, and it stretched every heading by a factor of
    /// `25600 / 11520`, putting a chest 40 degrees off at a heading of 90.
    fn spawn_position(&self, world: &World) -> [u32; 3] {
        const DISTANCE: f32 = 3.0 * world::POSITION_SCALE as f32;

        let Some(position) = self.position else {
            // The client has not said where it is yet. The world's spawn point is at least on
            // the island, where the origin is buried in terrain.
            let spawn = world.spawn_position();

            return [spawn[0], spawn[1], spawn[2] + world::POSITION_SCALE * 3];
        };

        let degrees = self.facing_yaw.unwrap_or(0) as f32 / ANGLE_UNITS_PER_DEGREE;
        let radians = degrees.to_radians();

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

        if opening || !self.raise_lid_on_close {
            self.closed_lids.remove(&target);
        } else {
            self.closed_lids.insert(target);
        }

        debug!(target, opening, "container");

        // **Both parameters, every time, the player first.**
        //
        // Measured against the running C# with `skysaga-probe`'s `close-burst` example, which
        // drives both servers through spawn/open/close and prints which parameters each sync
        // carries. The C# sends `usingentityid` then `hasbeenopened` on *both* presses, even
        // on a first open where the flag does not change: it assigns both unconditionally, and
        // its setter marks a parameter dirty on assignment rather than on change.
        //
        // Two earlier attempts got this wrong in opposite directions. One reversed the order
        // on a close, reasoning that the lid animation had to land before the window closed
        // and cleared the client's "window open" latch. The other dropped the chest sync from
        // a first open because nothing had changed. Neither shut the lid, and the second
        // quietly reintroduced the poisoned-open-path bug the notes warn about. The order and
        // the redundancy are now copied from the implementation that demonstrably works rather
        // than argued from the reversing notes.
        let mut out = self.sync_player(world);

        out.extend(self.sync_container(target, world));

        out
    }

    /// Whether a close raises `hasbeenopened`.
    ///
    /// A diagnostic lever, not a feature, in the same spirit as the C#'s `/useid`: the flag is
    /// documented as the close signal, and the chest is nonetheless left standing open after
    /// one. Turning it off distinguishes "the raised lid is what this flag means" from "the
    /// close event needs something else", without a rebuild between the two.
    pub fn set_raise_lid_on_close(&mut self, raise: bool) {
        self.raise_lid_on_close = raise;
    }

    pub fn raises_lid_on_close(&self) -> bool {
        self.raise_lid_on_close
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
        // Only a hand can be holding anything, and what it holds decides between three
        // outcomes: a block places a voxel, a device or a decoration places an *entity*, and
        // everything else digs. The hotbar keeps resource *hashes*, so it can still name an
        // item the player has run out of -- hence the check that a stack actually exists
        // before one is taken from it.
        let held = packet.location.is_hand().then(|| self.held_resource()).flatten();

        // An Anvil is not a placeable block, so before this branch existed it fell through to
        // the dig below and broke the ground it was clicked on.
        if let Some((item, entity)) =
            held.and_then(|held| Some((held, world.geodata.places_entity(held)?)))
        {
            return self.place_entity(item, entity, &packet, world);
        }

        let placing = held.and_then(|held| {
            world
                .geodata
                .placeable_for_hash(held)
                .map(|material| (held, material))
        });

        let Some((item, material)) = placing else {
            return self.dig(packet.chunk, packet.voxel, world);
        };

        // Taking from the stack also confirms there was one to take.
        let taken = self.take_one(item);

        if taken.is_empty() {
            debug!(item, "the hotbar names a block the player does not have");

            return self.dig(packet.chunk, packet.voxel, world);
        }

        // The new block goes into the empty voxel next to the face that was clicked, not into
        // the one that was hit.
        let voxel = packet.placement_voxel();

        world.set_block(packet.chunk, voxel, material);

        debug!(?packet.chunk, ?voxel, material, "place");

        // The block, then the stack it came out of. Both are needed: the client draws the
        // count it was last sent, so a placement that only reports the block leaves the
        // player holding an inexhaustible stack.
        let mut out = vec![Self::chunk_edit(packet.chunk, voxel, material)];

        out.extend(self.apply(taken, world));

        out
    }

    /// Put a device or a decoration in the world, out of the stack the player is holding.
    ///
    /// # The voxel packet is the whole placement
    ///
    /// `PerformVoxelActions` is the **only** packet a device placement sends. A live Anvil
    /// produced no `ExecuteEntityAction` at all, so there is no second half to wait for and
    /// nothing here needs the action row: which slot acted, which voxel and which face are all
    /// in the packet already.
    ///
    /// # The entity is the resource's own name
    ///
    /// The `Anvil` item places the `Anvil` entity, and the data says so only by way of
    /// `ActionVoxel`. The definition is looked up **before** the stack is touched, so a
    /// resource naming an entity the data file does not define places nothing rather than
    /// eating an item.
    fn place_entity(
        &mut self,
        item: u32,
        name: &str,
        packet: &PerformVoxelActions,
        world: &World,
    ) -> Vec<Vec<u8>> {
        let Some(definition) = world.definitions.get(name).cloned() else {
            warn!(%name, "places an entity the data file does not define");

            return Vec::new();
        };

        let taken = self.take_one(item);

        if taken.is_empty() {
            debug!(item, %name, "the hotbar names a device the player does not have");

            return self.dig(packet.chunk, packet.voxel, world);
        }

        // Next to the face that was clicked, as a block placement is: the clicked voxel is
        // solid, so an entity put there is inside it.
        let voxel = packet.placement_voxel();
        let position = World::voxel_corner(packet.chunk, voxel);

        let id = self.inventories.next_entity_id();
        self.inventories.reserve_ids_from(id + 1);

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
            world::device_components(position, links, definition.max_crafting_slots()),
        );

        // **On the world, not on this session.** An anvil one player puts down is an anvil
        // everybody can use, and it has to still be there tomorrow. See [`world::Device`].
        world.place_device(world::Device {
            id,
            name: name.to_owned(),
            entity: built,
        });

        info!(entity = id, %name, ?voxel, ?position, "placed a device");

        // The entity, then the stack it came out of -- the same order and the same reason as a
        // block: a placement that reports only the entity leaves the player holding an
        // inexhaustible stack.
        let mut out = Vec::new();

        if let Some((built, definition)) = self.entity_now(id, world) {
            let announcement = encode(|w| built.to_entity_add(definition).encode(w));

            // Everyone already in the world is told as well. Without this the anvil appears
            // for the other players only when they next log in, since the burst is the only
            // other thing that mentions it.
            self.broadcasts.push(announcement.clone());

            out.push(announcement);
        }

        out.extend(self.apply(taken, world));

        out
    }

    // --- the quest log ------------------------------------------------------------------

    /// Put an objective on the list and sync it back.
    ///
    /// **There is no reply packet.** `TodoListTaskAdd` is client-to-server only, and what
    /// confirms it is the `tasklist` sync, the same shape as the wallet, where the currency
    /// sync *is* the answer to `RechargeLifeTickets`.
    ///
    /// The id the client sends is not trusted. Ids are 8 bits over a list capped at 32 and the
    /// UI element is slot-indexed (`Todo_00_tutorial`), so they are the server's to allocate;
    /// honouring a client-chosen id lets two rows collide and makes Erase ambiguous.
    fn todo_add(&mut self, packet: TodoListTaskAdd, world: &World) -> Vec<Vec<u8>> {
        if self.todo_tasks.len() >= TASK_LIST_DEFAULT as usize {
            debug!("the quest log is full");

            return Vec::new();
        }

        let Some(task_id) = self.todo_list().free_id() else {
            return Vec::new();
        };

        let task = TodoTask {
            task_id,
            ..packet.task
        };

        debug!(
            task = task_id,
            manually_added = packet.manually_added,
            "todo task added",
        );

        self.todo_tasks.push(task);

        self.sync_of(self.player_entity_id, &["tasklist"], world)
            .into_iter()
            .collect()
    }

    /// Delete a task for good.
    fn todo_erase(&mut self, packet: TodoListTaskRef, world: &World) -> Vec<Vec<u8>> {
        let before = self.todo_tasks.len();

        self.todo_tasks.retain(|task| task.task_id != packet.task_id);

        if self.todo_tasks.len() == before {
            debug!(task = packet.task_id, "erasing a task that is not there");
        }

        // Synced even when nothing changed: the client has already taken the row off its own
        // list, so staying silent leaves the two disagreeing.
        self.sync_of(self.player_entity_id, &["tasklist"], world)
            .into_iter()
            .collect()
    }

    /// The quest log as a component, for syncing.
    fn todo_list(&self) -> skysaga_world::TodoListComponent {
        skysaga_world::TodoListComponent {
            tasks: self.todo_tasks.clone(),
        }
    }

    // --- crafting -----------------------------------------------------------------------

    /// Start a craft, or say why not.
    ///
    /// **Every refusal answers.** `CraftingFailed` is the only thing that takes the client out
    /// of its crafting state; refusing in silence leaves the panel spinning until the player
    /// closes and reopens it, which looks like the server having crashed.
    fn queue_recipe(&mut self, packet: QueueRecipeOnEntity, world: &World) -> Vec<Vec<u8>> {
        // **The refusal names what could not be made, not the recipe that would have made
        // it.** The client's handler resolves this field through the *resource* table
        // (`FUN_008787e0`) before it clears the crafting state, so a recipe id -- which is a
        // hash of a recipe name and resolves to no resource at all -- leaves the panel spinning
        // for ever. Observed live: a torch queued at an anvil was refused, the packet went out,
        // and the window stayed busy until the client was restarted.
        let output_of = |recipe: Option<&skysaga_world::geodata::Recipe>| {
            recipe
                .and_then(|recipe| recipe.output())
                .map(|(name, _)| skysaga_core::name_hash(name))
        };

        let refuse = |reason: &str, recipe: Option<&skysaga_world::geodata::Recipe>| {
            debug!(entity = packet.entity_id, item = ?packet.item_id, reason, "craft refused");

            vec![encode(|w| {
                CraftingFailed {
                    entity_id: packet.entity_id,
                    resource: output_of(recipe),
                }
                .encode(w)
            })]
        };

        let Some(item) = packet.item_id else {
            return refuse("no item named", None);
        };

        // The recipe is addressed by its own name's hash, not by what it makes.
        let Some(recipe) = world.geodata.recipe_for_id(item).cloned() else {
            return refuse("no such recipe", None);
        };

        // **The packet names whichever panel is open, which is not always the right station.**
        //
        // Observed live: with an anvil's window open, the client queued `Hand_Craft_Torch`
        // against the *anvil's* entity. The hand-crafting panel does not address the player's
        // own entity while a station is open -- it addresses the station -- so a server that
        // insists the two agree refuses every hand recipe the moment the player stands at an
        // anvil, which is most of the time.
        //
        // So a hand recipe is served wherever it is asked for: a pair of hands is available at
        // an anvil as much as anywhere else. Anything needing a real station still has to name
        // that station, which is what stops an anvil recipe being made in mid-air.
        let Some(station) = self.station_of(packet.entity_id, world) else {
            return refuse("not a crafting station", Some(&recipe));
        };

        let made_here =
            recipe.station() == Some(station.as_str()) || recipe.station() == Some(HAND_CRAFTING);

        if !made_here {
            return refuse("that recipe is not made here", Some(&recipe));
        }

        let queued = self.crafting.get(&packet.entity_id).map_or(0, Vec::len);

        if queued >= self.max_crafting_slots_of(packet.entity_id, world) as usize {
            return refuse("no free slot", Some(&recipe));
        }

        // Check the whole list before taking any of it, or a craft that runs out halfway
        // consumes the materials it did find.
        for (material, quantity) in recipe.materials() {
            if self.carried(skysaga_core::name_hash(material)) < quantity {
                return refuse("not enough materials", Some(&recipe));
            }
        }

        // **The clock, before the craft that depends on it.**
        //
        // A crafting slot carries the moment it started, and the client turns that into a
        // progress bar with its own `now()`. Until a `TimeSync` lands, that `now()` is
        // milliseconds since the client *launched* -- a number in the thousands -- so every
        // real timestamp is in its far future, progress stays at 0.0 for ever, and the panel
        // hangs holding the keyboard. Sending the clock once at handover is not enough: it is
        // one packet in the middle of a busy handshake, and a client that misses it can never
        // finish a craft again for the whole session. Measured live -- the same three crafts
        // completed or hung depending on the run, with byte-identical crafting traffic.
        //
        // The handler rebases exactly (`FUN_0089c620` stores the server's time *and* a fresh
        // local baseline), so re-sending costs eight bytes and cannot drift.
        let mut out = vec![encode(|w| {
            TimeSync {
                now_ms: self.now_ms(),
            }
            .encode(w)
        })];

        for (material, quantity) in recipe.materials() {
            out.extend(self.take(skysaga_core::name_hash(material), quantity, world));
        }

        let (output, _) = recipe.output().unwrap_or(("", 0));

        // **The moment it started, not zero.** The client's progress is
        // `(now - timer) / duration`, so a zero start time is one at the Unix epoch and every
        // craft read as finished the instant it was queued -- the recipe's three seconds
        // elapsed before the panel had drawn.
        let started = self.now_ms();

        self.crafting
            .entry(packet.entity_id)
            .or_default()
            .push(skysaga_world::CraftingSlot {
                recipe: Some(recipe.id()),
                output: Some(skysaga_core::name_hash(output)),
                timer: started,
                ready_ms: started + Self::duration_ms(&recipe),
                announced: false,
                materials: Vec::new(),
            });

        info!(recipe = %recipe.name, output, %station, "queued a craft");

        out.extend(self.sync_of(packet.entity_id, &["craftingslots"], world));

        // No "your item is ready" here: it is not. See [`Self::take_notifications`], which
        // sends one when the timer actually runs out.
        out
    }

    /// How long a craft takes, in milliseconds.
    ///
    /// The client derives its own figure from the recipe **and the materials chosen**, so the
    /// two never agree exactly; see [`COLLECT_GRACE_MS`].
    fn duration_ms(recipe: &skysaga_world::geodata::Recipe) -> u64 {
        (recipe.execution_time_seconds.max(0.0) * 1000.0) as u64
    }

    /// "Your item is ready", for every craft whose timer has just run out.
    ///
    /// **A toast, not a state change.** `FUN_0073b210` touches nothing but the notification
    /// queue, so this does not make a slot collectable -- the slot's own start time does that.
    /// Without it a finished craft still collects; the player is simply never told, which looks
    /// exactly like a craft that silently did nothing.
    fn crafting_announcements(&mut self) -> Vec<Vec<u8>> {
        if !craft_notification_enabled() {
            return Vec::new();
        }

        let now = self.now_ms();
        let mut out = Vec::new();

        for queue in self.crafting.values_mut() {
            for slot in queue.iter_mut() {
                if slot.announced || slot.ready_ms > now {
                    continue;
                }

                slot.announced = true;

                out.push(encode(|w| {
                    CraftingNotification {
                        resource: slot.output,
                        // **Both specs name the item.** Sent empty, this packet wedges the
                        // client's crafting panel: measured live, three consecutive crafts
                        // complete with the toast suppressed and the craft it announces hangs
                        // with it sent. The handler builds a notification record out of the
                        // resource *and* both specs, so an empty pair is the obvious suspect.
                        item_spec_a: ItemSpec {
                            resource: slot.output,
                            ..Default::default()
                        },
                        item_spec_b: ItemSpec {
                            resource: slot.output,
                            ..Default::default()
                        },
                    }
                    .encode(w)
                }));
            }
        }

        out
    }

    /// Take a finished craft out of its slot and into the rucksack.
    fn collect_craft(&mut self, packet: CollectCraftedItemInSlot, world: &World) -> Vec<Vec<u8>> {
        // Logged on arrival, not just on failure. Whether the client ever asks to collect is
        // the first question when a craft "does nothing", and a handler that is silent on the
        // happy path cannot answer it.
        debug!(
            station = packet.entity_id,
            slot = packet.slot,
            "collect requested"
        );

        let Some(slot) = self
            .crafting
            .get(&packet.entity_id)
            .and_then(|queue| queue.get(packet.slot as usize))
            .cloned()
        else {
            debug!(slot = packet.slot, "collecting an empty crafting slot");

            return Vec::new();
        };

        let Some(recipe) = slot.recipe.and_then(|id| world.geodata.recipe_for_id(id)) else {
            return Vec::new();
        };

        // **The server decides when a craft is done, not the client.**
        //
        // The client already refuses to offer a slot below 100% -- `FUN_008a7fa0` gates the
        // collect on its progress reaching 1.0 -- so an early request means either a modified
        // client or a clock the two disagree about. Either way, handing the item over would
        // make the recipe's `ExecutionTimeInSeconds` advisory, which is the whole point of it.
        //
        // Silent, because there is no "not yet" packet: `CraftingFailed` unsticks the crafting
        // *panel*, and sending one for a collect the player never consciously made would take
        // them out of a UI they are still using.
        let elapsed_ms = self.now_ms().saturating_sub(slot.timer);
        let full_ms = (recipe.execution_time_seconds.max(0.0) * 1000.0) as u64;
        let required_ms = full_ms.saturating_sub(COLLECT_GRACE_MS);

        if elapsed_ms < required_ms {
            debug!(
                slot = packet.slot,
                elapsed_ms, required_ms, full_ms, "collecting a craft that is not finished",
            );

            return Vec::new();
        }

        let (name, quantity) = recipe.output().unwrap_or(("", 1));
        let name = name.to_owned();

        if let Some(queue) = self.crafting.get_mut(&packet.entity_id) {
            queue.remove(packet.slot as usize);
        }

        let Some(stack) = self.give(&name, quantity) else {
            warn!(item = %name, "crafted, but the rucksack is full");

            // The slot is already gone, so say so; the item is lost, which is worse than
            // refusing would have been and is why a full rucksack should be checked earlier.
            return self.sync_of(packet.entity_id, &["craftingslots"], world)
                .into_iter()
                .collect();
        };

        let mut out = self.apply(
            vec![
                Effect::ItemCreated { entity: stack },
                Effect::SlotsChanged {
                    owner: self.player_entity_id,
                },
            ],
            world,
        );

        out.extend(self.sync_of(packet.entity_id, &["craftingslots"], world));

        info!(item = %name, quantity, "collected a craft");

        out
    }

    // --- the drop slots: repair and dismantle -------------------------------------------

    /// Drag an item out of the rucksack and onto a drop square.
    ///
    /// **The client does not move it itself.** It sends the packet and waits, so both
    /// `inventoryentitylist` and `craftingdropslots` have to be synced back or the square stays
    /// empty and the item stays in the bag -- the "UI looks frozen" failure that
    /// `InventoryItemSwap` documents.
    fn move_to_drop_slot(
        &mut self,
        packet: MoveItemToCraftingDropSlot,
        world: &World,
    ) -> Vec<Vec<u8>> {
        let Some(square) = self.drop_slot(packet.slot_type) else {
            return Vec::new();
        };

        // One item per square. The client will not offer a second, and quietly swapping would
        // leave the first nowhere: it is out of the rucksack while it sits here.
        if self.drop_slots[square] != 0 {
            debug!(square, "that drop square is already full");

            return Vec::new();
        }

        let Some((item, effects)) = self
            .inventories
            .detach(self.player_entity_id, packet.slot)
        else {
            debug!(slot = packet.slot, "dropped an empty square onto a drop square");

            return Vec::new();
        };

        self.drop_slots[square] = item;

        debug!(square, item, from = packet.slot, "moved onto a drop square");

        let mut out = self.apply(effects, world);

        out.extend(self.sync_drop_slots(world));

        out
    }

    /// Drag it back out again.
    ///
    /// Into the first free rucksack square, and if there is none the item **stays where it
    /// is**: dropping it on the floor or destroying it would lose something the player put
    /// down deliberately.
    fn take_from_drop_slot(&mut self, slot_type: u32, world: &World) -> Vec<Vec<u8>> {
        let Some(square) = self.drop_slot(slot_type) else {
            return Vec::new();
        };

        let item = self.drop_slots[square];

        if item == 0 {
            return Vec::new();
        }

        let effects = self.inventories.collect(self.player_entity_id, item);

        if effects.is_empty() {
            warn!(square, item, "nowhere to put it back; the rucksack is full");

            return Vec::new();
        }

        self.drop_slots[square] = 0;

        debug!(square, item, "taken back off a drop square");

        let mut out = self.apply(effects, world);

        out.extend(self.sync_drop_slots(world));

        out
    }

    /// Press repair, or dismantle.
    ///
    /// # Repair is honest about doing nothing
    ///
    /// The server does not model item durability -- an item entity is a name and a count -- so
    /// there is nothing to restore. The item goes back to the rucksack and the panel is told
    /// the job is done, which is what the client needs to leave its busy state. When durability
    /// arrives this is the one line that changes.
    ///
    /// # Dismantle gives the recipe's materials back
    ///
    /// The item is taken apart into whatever the recipe that builds it consumed. Only one
    /// output's worth is consumed at a time, which matches the client's own precondition: it
    /// refuses the drop when the stack is smaller than the recipe's output quantity.
    fn perform_drop_slot_action(&mut self, slot_type: u32, world: &World) -> Vec<Vec<u8>> {
        let Some(square) = self.drop_slot(slot_type) else {
            return Vec::new();
        };

        let item = self.drop_slots[square];

        if item == 0 {
            debug!(square, "the button was pressed with nothing on the square");

            return Vec::new();
        }

        match square {
            DISMANTLE_SLOT => self.dismantle(item, world),

            // Repair. Nothing to restore, so the item simply comes back.
            REPAIR_SLOT => {
                let resource = self.inventories.name(item);

                let mut out = self.take_from_drop_slot(slot_type, world);

                out.push(encode(|w| {
                    CraftingNotification {
                        resource,
                        ..Default::default()
                    }
                    .encode(w)
                }));

                info!(item, "repaired, as far as anything is damaged");

                out
            }

            // Unreachable: `drop_slot` has already refused anything outside the two squares.
            _ => Vec::new(),
        }
    }

    /// Take the item on the dismantle square apart, and hand its materials back.
    fn dismantle(&mut self, item: u32, world: &World) -> Vec<Vec<u8>> {
        let Some(resource) = self.inventories.name(item) else {
            return Vec::new();
        };

        // The recipe that **builds** this item is what says what it is made of. Repair recipes
        // (`RecipeType` 0) are excluded: their output is a repaired entity rather than a thing
        // with materials in it.
        let Some(recipe) = world
            .geodata
            .recipes()
            .iter()
            .find(|recipe| {
                recipe.recipe_type == 1
                    && recipe
                        .output()
                        .is_some_and(|(name, _)| skysaga_core::name_hash(name) == resource)
            })
            .cloned()
        else {
            debug!(item, "nothing makes that, so nothing takes it apart");

            return Vec::new();
        };

        let (_, made) = recipe.output().unwrap_or(("", 1));

        let effects = self.inventories.consume_loose(item, made.max(1));

        if effects.is_empty() {
            debug!(item, made, "not enough of it to dismantle one");

            return Vec::new();
        }

        if self.inventories.item(item).is_none() {
            self.drop_slots[DISMANTLE_SLOT] = 0;
        }

        let mut out = self.apply(effects, world);

        // The materials, as new stacks in the rucksack. A full rucksack loses them, which is
        // the same trade-off a collected craft makes and the same place a check belongs when
        // one is added.
        for (material, quantity) in recipe.materials() {
            let material = material.to_owned();

            let Some(stack) = self.give(&material, quantity) else {
                warn!(item = %material, "dismantled, but the rucksack is full");

                continue;
            };

            out.extend(self.apply(
                vec![
                    Effect::ItemCreated { entity: stack },
                    Effect::SlotsChanged {
                        owner: self.player_entity_id,
                    },
                ],
                world,
            ));
        }

        info!(%recipe.name, "dismantled");

        out.extend(self.sync_drop_slots(world));

        out
    }

    /// Which square a `slotType` names, or `None` for one that does not exist.
    ///
    /// The field is two bits wide because there are two squares, not because indices go up to
    /// three -- so 2 and 3 are values a well-behaved client never sends.
    fn drop_slot(&self, slot_type: u32) -> Option<usize> {
        let square = slot_type as usize;

        (square < DROP_SLOTS).then_some(square).or_else(|| {
            debug!(slot_type, "no such crafting drop square");

            None
        })
    }

    /// Tell the client what is on the drop squares now.
    fn sync_drop_slots(&self, world: &World) -> Vec<Vec<u8>> {
        self.sync_of(self.player_entity_id, &["craftingdropslots"], world)
            .into_iter()
            .collect()
    }

    /// What is on the two drop squares, by item entity id.
    pub fn drop_slots(&self) -> [u32; DROP_SLOTS] {
        self.drop_slots
    }

    /// The station `entity` is, by the resource name a recipe would call it.
    ///
    /// **`Hand_Crafting` is a station like any other.** It is a real resource name, it appears
    /// in the client, and 17 recipes name it as their non-expendable input -- so the player's
    /// own entity answers with it and hand crafting needs no special case beyond this line.
    ///
    /// `None` for anything that is not a station, which is what refuses a craft queued against
    /// a sheep.
    fn station_of(&self, entity: u32, world: &World) -> Option<String> {
        if entity == self.player_entity_id {
            return Some(HAND_CRAFTING.to_owned());
        }

        // A placed device is named by the resource that placed it, and a recipe's station is
        // that same name -- `Anvil` the item, `Anvil` the entity, `Anvil` the input.
        if let Some(device) = world.device(entity) {
            return world
                .definitions
                .get(&device.name)
                .and_then(|definition| definition.max_crafting_slots())
                .map(|_| device.name);
        }

        let container = self.container(entity, world)?;

        container
            .definition
            .max_crafting_slots()
            .map(|_| container.name.clone())
    }

    /// How long `entity`'s crafting queue is, from the entity's own data.
    ///
    /// A station's queue length is a property of the station: an `Anvil` takes three crafts at
    /// once, a pair of hands one. Reading it from the definition rather than assuming keeps a
    /// placed station's window agreeing with the server about how many slots it has, and the
    /// client draws exactly as many squares as `maxcraftingslots` says.
    ///
    /// Zero for an entity that is not a station at all, which refuses a craft queued against
    /// a sheep without a special case for it.
    pub fn max_crafting_slots_of(&self, entity: u32, world: &World) -> u8 {
        self.entity_now(entity, world)
            .and_then(|(_, definition)| definition.max_crafting_slots())
            .unwrap_or(0)
    }

    /// Where a placed device stands, in the client's position units.
    pub fn device_position(&self, entity: u32, world: &World) -> Option<[u32; 3]> {
        if let Some(position) = world.device_position(entity) {
            return Some(position);
        }

        let container = self.container(entity, world)?;

        container.entity.components.iter().find_map(|component| {
            match component {
                Component::Transform(transform) => Some(transform.position),
                _ => None,
            }
        })
    }

    /// How many of `hash` the player is carrying, across every stack.
    fn carried(&self, hash: u32) -> u32 {
        (0..self.inventory().len() as u32)
            .filter_map(|slot| self.inventories.slot(self.player_entity_id, slot))
            .filter(|item| *item != 0)
            .filter(|item| self.inventories.name(*item) == Some(hash))
            .filter_map(|item| self.inventories.count(item))
            .sum()
    }

    /// Take `quantity` of `hash` out of the rucksack, and say what the client must be told.
    fn take(&mut self, hash: u32, quantity: u32, world: &World) -> Vec<Vec<u8>> {
        let mut out = Vec::new();

        for _ in 0..quantity {
            let taken = self.take_one(hash);

            if taken.is_empty() {
                break;
            }

            out.extend(self.apply(taken, world));
        }

        out
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
    ///
    /// A finished craft is checked for here rather than pushed from a timer: the server has no
    /// per-session tick, and this is called after every packet a client sends -- which, with
    /// movement arriving several times a second, is as good as one.
    pub fn take_notifications(&mut self) -> Vec<Vec<u8>> {
        let mut out = self.crafting_announcements();

        out.append(&mut self.notifications);

        out
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

    /// What block stands at a voxel now: what players have done to it, or the world as built.
    ///
    /// The edits belong to the **world**, so a block one player places is a block every player
    /// digs. They come first, so a block placed and then dug drops the thing that was placed
    /// rather than the terrain that used to be underneath it.
    fn material_at(&self, chunk: [u32; 3], voxel: [u32; 3], world: &World) -> u8 {
        world.block_at(chunk, voxel)
    }

    /// One dig tick on a voxel. The block gives way once enough of them land, and leaves
    /// behind whatever it was made of.
    ///
    /// **The three crack stages the player sees are client-side.** It streams one packet per
    /// tick, every one identical, and the server counts them and decides. Breaking on the
    /// first would make every block give way three times too fast, which is the sort of
    /// difference that is invisible in a unit test and obvious in the game.
    ///
    /// What it drops lands in the **middle of the hole**, not at the packet's `hit`.
    ///
    /// `hit` is tempting: it is already in position units, so it needs no scale chosen. But it
    /// is where the tool *touched*, which is a point on the face of the block, so a drop placed
    /// there hangs against the side of the hole or on top of it. In front of a client that
    /// reads as an item floating at head height. The voxel's own centre is the position the
    /// block occupied, and the half-voxel lift is the one creature loot already uses to keep a
    /// pickup out of the ground.
    fn dig(&mut self, chunk: [u32; 3], voxel: [u32; 3], world: &World) -> Vec<Vec<u8>> {
        let material = self.material_at(chunk, voxel, world);

        // Air, bedrock and water. The client raycasts to a solid block before it sends
        // anything, so refusing here costs an honest dig nothing, and it is what stops a swing
        // at the sky reporting a hole.
        if !world.geodata.is_diggable(material) {
            debug!(?chunk, ?voxel, material, "not diggable");

            return Vec::new();
        }

        let ticks = self.dig_damage.entry((chunk, voxel)).or_insert(0);

        *ticks += 1;

        if *ticks < DIG_TICKS_TO_BREAK {
            debug!(?chunk, ?voxel, ticks = *ticks, "dig tick");

            return Vec::new();
        }

        self.dig_damage.remove(&(chunk, voxel));

        world.set_block(chunk, voxel, PartialChunkEditsSync::AIR);

        // The hole first, then what fell out of it. The other order puts the item inside a
        // block the client still believes is solid.
        let mut out = vec![Self::chunk_edit(chunk, voxel, PartialChunkEditsSync::AIR)];

        match world.geodata.item_for_voxel(material) {
            Some(item) => {
                debug!(?chunk, ?voxel, material, %item, "dug through");

                out.extend(self.drop_pickup(&item, 1, World::voxel_centre(chunk, voxel), world));
            }

            // A block with no item form. `Tree` is the one in this data: it breaks and yields
            // nothing, which is the data's answer rather than a lookup failure.
            None => debug!(
                ?chunk,
                ?voxel,
                material,
                "dug through, and it yields nothing"
            ),
        }

        out
    }

    /// Take one item of `hash` out of the rucksack, and say what the client must be told.
    ///
    /// **Empty means nothing was taken**, which is how a caller tells "the hotbar names an item
    /// the player has run out of" from a successful take.
    ///
    /// The effects have to be returned rather than dropped. This used to answer `bool` and
    /// throw them away, so the server counted a stack down while the client went on drawing
    /// the number it was last sent: blocks looked infinite, and once the server reached zero a
    /// placement quietly turned into a dig.
    fn take_one(&mut self, hash: u32) -> Vec<Effect> {
        let Some(slot) = (0..self.inventory().len() as u32).find(|slot| {
            self.inventories
                .slot(self.player_entity_id, *slot)
                .filter(|item| *item != 0)
                .and_then(|item| self.inventories.name(item))
                == Some(hash)
        }) else {
            return Vec::new();
        };

        self.inventories.destroy(self.player_entity_id, slot, 1)
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

                    // The queue is the player's own, which is what hand crafting is.
                    Component::Crafting(crafting) => {
                        crafting.slots = self.queue_of(entity);
                    }

                    // What is sitting on the repair and dismantle squares.
                    Component::CraftingDropSlots(drop_slots) => {
                        drop_slots.slots = self.drop_slots.to_vec();
                    }

                    // The quest log likewise: the world's template carries an empty one.
                    Component::TodoList(todo) => {
                        todo.tasks = self.todo_tasks.clone();
                    }

                    // The template carries full health; what this player has left is here.
                    Component::Health(health) => {
                        *health = HealthComponent::with_health(self.player_health());
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

        // A creature, whose only mutable state is what is left of it. Like a device, it lives
        // behind the world's lock, so its definition is looked up by name from the table that
        // outlives the lock rather than borrowed from the creature itself.
        if let Some(creature) = world.creature_now(entity) {
            let definition = world.definitions.get(&creature.name)?;

            let health = creature.max_health.saturating_sub(world.damage_to(entity));

            let mut built = creature.entity.clone();

            for component in &mut built.components {
                if let Component::Health(current) = component {
                    *current = HealthComponent::with_health(health);
                }
            }

            return Some((built, definition));
        }

        // A device a player placed. It lives on the world rather than on any session, so its
        // definition is looked up by name instead of being carried alongside: the definition
        // table outlives the lock the device comes out of.
        if let Some(device) = world.device(entity) {
            let definition = world.definitions.get(&device.name)?;

            let mut built = device.entity.clone();

            self.fill_container_components(entity, &mut built);

            return Some((built, definition));
        }

        let container = self.container(entity, world)?;

        let mut built = container.entity.clone();

        self.fill_container_components(entity, &mut built);

        Some((built, &container.definition))
    }

    /// Fill in the parts of a chest or a station that change while the server runs.
    ///
    /// The entity was built once, when the world seeded it or a player placed it; what is in
    /// it, whether its lid has been lifted and what it is making are all this session's.
    fn fill_container_components(&self, entity: u32, built: &mut Entity) {
        for component in &mut built.components {
            match component {
                Component::Inventory(inventory) => {
                    inventory.inventory_entity_list = self.inventories.slots(entity).to_vec();
                }

                Component::Interaction(interaction) => {
                    interaction.has_been_opened = self.closed_lids.contains(&entity);
                }

                // A station's queue, which is what its window draws. The entity was built
                // empty when it was placed, and everything queued since lives on the session.
                Component::Crafting(crafting) => {
                    crafting.slots = self.queue_of(entity);
                }

                _ => {}
            }
        }
    }

    // --- combat --------------------------------------------------------------------------

    /// The creature with this entity id, whether the world seeded it or a command spawned it.
    ///
    /// The session first, for the same reason as a container: the world is fixed once built,
    /// so anything created while the server runs lives here.
    pub fn creature(&self, id: u32, world: &World) -> Option<world::Creature> {
        world.creature_now(id)
    }

    /// Hit points left on a creature, or `None` if that id is not one.
    ///
    /// Takes the world because that is where creatures live: what is left of one is a fact
    /// about the world and not about whoever is asking.
    pub fn creature_health(&self, id: u32, world: &World) -> Option<u32> {
        let creature = world.creature_now(id)?;

        Some(creature.max_health.saturating_sub(world.damage_to(id)))
    }

    /// Hit points left on this player.
    pub fn player_health(&self) -> u32 {
        self.player_max_health().saturating_sub(self.player_damage)
    }

    /// This player's full health, from `Durabilities > Player`.
    ///
    /// A constant rather than a lookup: the world is not in scope everywhere this is needed,
    /// and every player in every build of 10414 resolves to the same row.
    pub fn player_max_health(&self) -> u32 {
        PLAYER_HEALTH
    }

    /// What is queued at `station`, or waiting to be collected there.
    pub fn crafting_queue(&self, station: u32) -> &[skysaga_world::CraftingSlot] {
        self.crafting
            .get(&station)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// What the player has on the go in their own hands.
    pub fn crafting_slots(&self) -> &[skysaga_world::CraftingSlot] {
        self.crafting_queue(self.player_entity_id)
    }

    /// A station's queue as the wire wants it, ready to be written into its component.
    fn queue_of(&self, station: u32) -> Vec<skysaga_world::CraftingSlot> {
        self.crafting_queue(station).to_vec()
    }

    /// The quest log's rows, in the order the client shows them.
    pub fn todo_tasks(&self) -> &[TodoTask] {
        &self.todo_tasks
    }

    /// Milliseconds since the Unix epoch, which is the clock the client reads.
    ///
    /// `FUN_0089c6b0` builds it from `GetSystemTimeAsFileTime` against a `FILETIME` for 1970,
    /// so this is the same scale the crafting slot's start time is compared against.
    fn now_ms(&self) -> u64 {
        if let Some(fixed) = self.clock_ms {
            return fixed;
        }

        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Pin the wall clock, so a test that crafts does not depend on how fast it runs.
    ///
    /// Craft timings are the only thing here that reads the time of day. Advance this rather
    /// than sleeping: a three-second recipe would otherwise cost three seconds per test.
    pub fn set_clock_ms(&mut self, now: u64) {
        self.clock_ms = Some(now);
    }

    /// Items lying on the floor, as `(pickup entity, stack entity)`.
    ///
    /// Takes the world because the floor is the world's: see [`World::floor_drops`].
    pub fn floor_drops_in(&self, world: &World) -> Vec<(u32, u32)> {
        world
            .floor_drops()
            .iter()
            .map(|drop| (drop.pickup, drop.stack))
            .collect()
    }

    /// Where a floor drop is lying, in position units of 1/64 of a voxel.
    pub fn floor_drop_position(&self, pickup: u32, world: &World) -> Option<[u32; 3]> {
        world
            .floor_drops()
            .iter()
            .find(|drop| drop.pickup == pickup)
            .map(|drop| drop.position)
    }

    /// Packets addressed to the other connections, if any. Drains.
    pub fn take_broadcasts(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.broadcasts)
    }

    /// Put a creature in the world, three voxels in front of the player.
    pub fn spawn_creature(&mut self, world: &World, entity: &str) -> Option<Spawned> {
        let position = self.spawn_position(world);

        self.spawn_creature_at(world, entity, position)
    }

    /// Put a creature at a known place.
    ///
    /// `None` when the data file defines no such entity, or when it defines one with no
    /// physical properties -- a tree is an entity too, and one with no health is not something
    /// this can put in a fight.
    pub fn spawn_creature_at(
        &mut self,
        world: &World,
        entity: &str,
        position: [u32; 3],
    ) -> Option<Spawned> {
        let definition = world.definitions.get(entity)?.clone();

        let max_health = world::health_of(&definition, &world.geodata).or_else(|| {
            warn!(%entity, "no physical properties, so no health; not spawning it");

            None
        })?;

        let id = self.inventories.next_entity_id();
        self.inventories.reserve_ids_from(id + 1);

        let built = Entity::new(
            id,
            world::creature_components(position, HealthComponent::with_health(max_health)),
        );

        let packets = vec![encode(|w| built.to_entity_add(&definition).encode(w))];

        info!(entity = id, %entity, max_health, ?position, "spawned a creature");

        // **On the world, not on this session.** One knight, one pool of hit points: a
        // creature per connection let two players kill the same thing twice and collect its
        // loot twice.
        world.spawn_creature(world::Creature {
            id,
            name: entity.to_owned(),
            entity: built,
            definition,
            position,
            max_health,
        });

        // Everyone already in the world sees it appear. A joiner gets it from the burst.
        self.broadcasts.extend(packets.iter().cloned());

        Some(Spawned {
            entity: id,
            position,
            packets,
        })
    }

    /// The attack button went down: remember what is being swung, and echo it.
    ///
    /// **No damage happens here.** This packet says what is in use, not what it touched --
    /// the client's own hit detection reports that separately, in
    /// [`PerformEntityActions`](Self::perform_entity_action). The action is held per equip
    /// slot because that slot id is the only thing joining the two.
    fn equipped_item_used(&mut self, packet: EquippedItemUsed, world: &World) -> Vec<Vec<u8>> {
        // Everyone else sees the swing whether or not it connects -- a miss is still an
        // animation.
        self.broadcasts.push(encode(|w| {
            EntityUsedEquippedItem {
                entity_id: self.player_entity_id,
                location: packet.location,
                equipped_action: packet.equipped_action,
                action_type: packet.action_type,
            }
            .encode(w)
        }));

        self.armed.remove(&packet.location);

        let Some(hash) = packet.equipped_action else {
            return Vec::new();
        };

        let Some(action) = world.geodata.equipped_action(hash).cloned() else {
            debug!(hash, "a swing naming an action this data file does not have");

            return Vec::new();
        };

        debug!(
            location = packet.location,
            action = %action.name,
            damage = action.attack_strength,
            "armed",
        );

        self.armed.insert(packet.location, action);

        Vec::new()
    }

    /// The swing connected: apply what the armed action is worth to the entity named.
    ///
    /// # The client decided this, not the server
    ///
    /// `PerformEntityActions` carries the entity its own hit detection resolved, which is
    /// better than anything the server can work out: it knows the swing's real arc, the
    /// animation's active window and where both bodies are between position updates. The
    /// server checks only that the target is a living creature within a plausible distance,
    /// which is the difference between trusting a client and taking dictation from one.
    ///
    /// # Order matters
    ///
    /// The victim's `EntitySync` goes out before its `KillOccurred`, and the `EntityRemoved`
    /// after both. The client bails out of `KillOccurred` entirely if the victim does not
    /// resolve, so removing the corpse first would swallow the kill feed.
    fn perform_entity_action(
        &mut self,
        packet: PerformEntityActions,
        world: &World,
    ) -> Vec<Vec<u8>> {
        let Some(action) = self.armed.get(&packet.location).cloned() else {
            // A hit from a slot nothing armed. Dropped rather than guessed at: assuming a
            // default action here would make an unarmed hand hit as hard as a sword.
            debug!(location = packet.location, "a hit with nothing armed");

            return Vec::new();
        };

        // Not every use is an attack: placing a block, eating and opening a portal all arm the
        // same slot, and none of them names an `ActionEntity`.
        if action.attack_strength == 0 {
            debug!(action = %action.name, "not an attack");

            return Vec::new();
        }

        let Some(creature) = world.creature_now(packet.entity_id) else {
            // A chest, a tree, another player, or an id belonging to nothing.
            debug!(target = packet.entity_id, "hit something that is not a creature");

            return Vec::new();
        };

        if world.is_dead(creature.id) || creature.max_health <= world.damage_to(creature.id) {
            debug!(target = creature.id, "hit a corpse");

            return Vec::new();
        }

        // The one thing the server does check, and deliberately a loose one: a claim from the
        // far side of the island is refused and nothing finer. See `combat::MAX_HIT_DISTANCE`
        // for why a tight bound here threw away two hits in five of a real fight.
        if let Some(position) = self.position {
            let attacker = combat::Attacker {
                position,
                reach: self.player_reach(world),
            };

            if !combat::in_range(&attacker, creature.position) {
                warn!(
                    target = creature.id,
                    distance = combat::distance(position, creature.position),
                    limit = combat::MAX_HIT_DISTANCE,
                    "refusing a hit from implausibly far away",
                );

                return Vec::new();
            }
        }

        self.hit(&creature, &action, packet.position, world)
    }

    /// Take one swing's damage off `creature` and say what to send.
    fn hit(
        &mut self,
        creature: &world::Creature,
        action: &EquippedAction,
        where_it_landed: [u32; 3],
        world: &World,
    ) -> Vec<Vec<u8>> {
        let target = creature.id;

        let before = creature.max_health.saturating_sub(world.damage_to(target));
        let after = before.saturating_sub(action.attack_strength);

        world.set_damage(target, creature.max_health.saturating_sub(after));

        info!(
            target,
            creature = %creature.name,
            action = %action.name,
            damage = before - after,
            health = after,
            // In voxels, from the attacker's last reported position. Only ever logged on a
            // *refusal* before, which made the one check this server has never been verified in
            // a real fight: a swing that connects is exactly the case worth measuring. The
            // reach that matters is a sword's, so these should read as single digits; the
            // doubled-scale fight that first exposed the position units logged 4.8 to 17.1.
            distance = self
                .position
                .map(|position| combat::distance(position, creature.position)),
            "hit",
        );

        // The hit spark and the floating number. `amount` is in the same half-heart units the
        // health component uses, which is what its shared field width says.
        //
        // Drawn where the client said the blow landed rather than at the creature's origin,
        // which is at its feet: a spark under a knight reads as a miss.
        let mut out = vec![encode(|w| {
            EventEffect {
                effect_type: HIT_EFFECT,
                entity_a: self.player_entity_id,
                entity_b: target,
                resource_a: None,
                resource_b: None,
                position: where_it_landed,
                direction: EventEffect::direction_from([0.0, 1.0, 0.0]),
                amount: (before - after) / 2,
            }
            .encode(w)
        })];

        // ...and the heart bar itself, which is an ordinary parameter sync. Everyone sees it:
        // the knight is one knight, so its hearts have to move on every screen.
        let health_sync = self.sync_health(target, world);

        self.broadcasts.extend(health_sync.iter().cloned());

        out.extend(health_sync);

        if after > 0 {
            // Still alive, so it may still give something up: shearing. Only a sheep has a
            // `_Hit_` table, and for everything else this rolls nothing.
            out.extend(self.award_hit_loot(&creature, world));
        }

        // **Once.** Two players swinging at the same knight can both land what looks like a
        // killing blow; the world decides which of them actually did, and the loser's swing
        // rolls no loot and announces no kill.
        if after == 0 && world.mark_dead(target) {
            let kill = encode(|w| {
                KillOccurred {
                    killer: self.player_entity_id,
                    victim: target,
                    weapon: None,
                }
                .encode(w)
            });

            let removal = encode(|w| EntityRemoved { entity_id: target }.encode(w));

            out.push(kill.clone());

            // Rolled on the killing blow and only then, so a corpse cannot be farmed.
            out.extend(self.award_loot(&creature, world));

            // Only after the kill: the client resolves the victim before it draws anything.
            out.push(removal.clone());

            // The other players watch it die too, or it stands there at full health on their
            // screens until they next log in.
            self.broadcasts.push(kill);
            self.broadcasts.push(removal);
        }

        out
    }

    /// Roll what `creature` was carrying and drop it on the floor where it died.
    ///
    /// # On the floor, not into the rucksack
    ///
    /// Loot lands as `Pickup` entities the player walks over, which is what the real game does
    /// and what the C# does for ore seams. Putting it straight into the rucksack was the first
    /// version of this and it is wrong twice over: the kill has no visible result, and the
    /// player is handed items they never touched.
    ///
    /// # Nothing to drop is an ordinary outcome
    ///
    /// Most of the bestiary has a table; the dinosaurs and the test entities have none, and a
    /// table whose entries all fail their chance rolls yields nothing either.
    fn award_loot(&mut self, creature: &world::Creature, world: &World) -> Vec<Vec<u8>> {
        let dropped = world
            .geodata
            .loot_for(&creature.name, &mut self.loot_rolls);

        if dropped.is_empty() {
            debug!(creature = %creature.name, "dropped nothing");

            return Vec::new();
        }

        let mut out = Vec::new();

        for (index, (item, count)) in dropped.into_iter().enumerate() {
            // Spread the drops so two stacks are not inside one another, and lift them clear
            // of the ground: a pickup at the corpse's own feet is inside the terrain.
            let at = [
                creature.position[0] + (index as u32 * world::POSITION_SCALE) / 2,
                creature.position[1] + world::POSITION_SCALE / 2,
                creature.position[2],
            ];

            out.extend(self.drop_pickup(&item, count, at, world));

            info!(creature = %creature.name, %item, count, ?at, "loot");
        }

        out
    }

    /// Roll what `creature` gives up for a non-fatal hit, and drop it.
    ///
    /// # Shearing
    ///
    /// A sheep has two tables: `NPC_Sheep_LootTable` for killing it, and
    /// `NPC_Sheep_Hit_LootTable` for hitting it -- the wool without the mutton. It is the only
    /// `_Hit_` table in the data, so in practice this is "hit a sheep, get wool" and a no-op
    /// for everything else.
    ///
    /// **This is farmable, and the data does not say whether it should be.** Nothing in the
    /// table carries a cooldown or a sheared flag, so a player can stand and stab one sheep
    /// forever. The literal reading of the table is implemented rather than a limit invented;
    /// if evidence for one turns up, it belongs here.
    fn award_hit_loot(&mut self, creature: &world::Creature, world: &World) -> Vec<Vec<u8>> {
        let dropped = world
            .geodata
            .hit_loot_for(&creature.name, &mut self.loot_rolls);

        let mut out = Vec::new();

        for (item, count) in dropped {
            let at = [
                creature.position[0],
                creature.position[1] + world::POSITION_SCALE / 2,
                creature.position[2],
            ];

            out.extend(self.drop_pickup(&item, count, at, world));

            info!(creature = %creature.name, %item, count, "sheared");
        }

        out
    }

    /// Put `count` of `item` on the floor at `at`, and say what to send.
    ///
    /// **Two entities, stack first.** The `Pickup` names the stack by id, so a client told
    /// about the pickup before the stack it points at has a pickup referencing nothing.
    fn drop_pickup(
        &mut self,
        item: &str,
        count: u32,
        at: [u32; 3],
        world: &World,
    ) -> Vec<Vec<u8>> {
        let stack = self
            .inventories
            .create_loose(skysaga_core::name_hash(item), count);

        self.inventories.reserve_ids_from(self.inventories.next_entity_id());

        let mut out = self.apply(vec![Effect::ItemCreated { entity: stack }], world);

        if out.is_empty() {
            // No `BasicInventoryItem` definition, so the stack cannot be serialised and a
            // pickup naming it would be pointing at nothing.
            warn!(%item, "cannot announce a floor drop");

            return Vec::new();
        }

        let id = self.inventories.next_entity_id();
        self.inventories.reserve_ids_from(id + 1);

        let Some(definition) = world.definitions.get(PICKUP).cloned() else {
            warn!("Entities.json defines no Pickup; loot cannot be dropped");

            return Vec::new();
        };

        let built = Entity::new(id, world::pickup_components(stack, at));

        let announcement = encode(|w| built.to_entity_add(&definition).encode(w));

        out.push(announcement.clone());

        // **On the world.** A pile of dirt on the ground is on everybody's ground: the other
        // players have to see it, and whoever walks over it first gets it.
        world.drop_item(world::FloorDrop {
            pickup: id,
            stack,
            item: skysaga_core::name_hash(item),
            count,
            position: at,
        });

        // The stack first and the pickup second, to everyone else as well: a pickup naming an
        // entity the client has not been told about draws nothing at all.
        self.broadcasts.extend(out.iter().cloned());

        out
    }

    /// Put an item on the floor and answer the `Pickup` entity, for tests and for commands.
    pub fn drop_item(
        &mut self,
        item: &str,
        count: u32,
        at: [u32; 3],
        world: &World,
    ) -> Option<u32> {
        self.drop_pickup(item, count, at, world);

        world.floor_drops().last().map(|drop| drop.pickup)
    }

    /// Make sure this session's model holds the stack a floor drop points at.
    ///
    /// A drop another player made was minted in *their* inventory model. The ids belong to the
    /// world, so the same stack is recreated here under the same id rather than allocated
    /// afresh; see [`skysaga_world::Inventories::create_loose_with_id`].
    fn ensure_stack(&mut self, drop: &world::FloorDrop) {
        self.inventories
            .create_loose_with_id(drop.stack, drop.item, drop.count);
    }

    /// Collect a floor drop into the rucksack.
    ///
    /// The client fires `ResourcePickupAction` at the pickup while the player stands on it --
    /// **repeatedly**, a dozen times for one item. So the pickup is removed on the first one,
    /// and every later arrival finds nothing and does nothing.
    ///
    /// The stack entity is already known to the client, so this sends a slot list and not an
    /// `EntityAdd`. That is the whole difference between collecting a drop and being given an
    /// item.
    fn collect_pickup(&mut self, pickup: u32, world: &World) -> Vec<Vec<u8>> {
        // Taken off the floor first, so that two players standing on the same pile cannot both
        // be handed it: whoever gets here first has it, and the other finds nothing.
        let Some(drop) = world.take_floor_drop(pickup) else {
            // Not a pickup, or already taken.
            return Vec::new();
        };

        // It may be another player's drop, in which case this session has never held the stack.
        self.ensure_stack(&drop);

        let stack = drop.stack;

        let effects = self.inventories.collect(self.player_entity_id, stack);

        if effects.is_empty() {
            warn!(pickup, "cannot collect: the rucksack is full");

            // Left on the floor deliberately: destroying it would lose the item.
            world.drop_item(drop);

            return Vec::new();
        }

        info!(pickup, stack, "collected");

        let mut out = self.apply(effects, world);

        // ...and the pickup itself goes away, or the client keeps drawing it and keeps asking
        // to collect it.
        out.push(encode(|w| EntityRemoved { entity_id: pickup }.encode(w)));

        out
    }

    /// How far this player can reach, from `Durabilities`-adjacent `Reaches`.
    fn player_reach(&self, world: &World) -> f32 {
        world
            .definitions
            .get("Player")
            .and_then(|definition| definition.physical_properties())
            .and_then(|properties| world.geodata.reach_for(properties))
            .unwrap_or(DEFAULT_REACH)
    }

    /// Sync an entity's hearts. Works for a creature and for the player alike.
    fn sync_health(&self, entity: u32, world: &World) -> Vec<Vec<u8>> {
        self.sync_of(entity, &["wholehearts", "halfhearts"], world)
            .into_iter()
            .collect()
    }

    /// Take health off this player, and say what to send.
    ///
    /// Falling is the one thing the client decides for itself: it reports `IFellTooFar` as a
    /// fact and waits to be told what it cost. **The cost is ours to choose** -- the packet
    /// carries no height and the client's own reaction to it is an empty function.
    fn hurt_player(&mut self, damage: u32, world: &World) -> Vec<Vec<u8>> {
        if self.player_health() == 0 {
            // Already dead and waiting on a respawn. A second fall must not raise a second
            // death screen.
            return Vec::new();
        }

        self.player_damage = (self.player_damage + damage).min(self.player_max_health());

        let mut out = self.sync_health(self.player_entity_id, world);

        if self.player_health() == 0 {
            info!("the player died");

            // Killer and victim alike: nothing else did this, and the client renders a
            // self-kill as the death screen the same way.
            out.push(encode(|w| {
                KillOccurred {
                    killer: self.player_entity_id,
                    victim: self.player_entity_id,
                    weapon: None,
                }
                .encode(w)
            }));
        }

        out
    }

    /// Put the player back on their feet, at the world's spawn point.
    ///
    /// **`PlayerSpawned` is the only code path that closes the death screen.** It also
    /// teleports and resets the camera, which is why it is the answer to falling off the world
    /// as well as to pressing respawn.
    fn respawn(&mut self, world: &World) -> Vec<Vec<u8>> {
        self.player_damage = 0;

        let position = world.spawn_position();

        self.position = Some(position);

        info!(?position, "respawning the player");

        let mut out = vec![encode(|w| {
            PlayerSpawned::at(self.player_entity_id, position, 0.0).encode(w)
        })];

        out.extend(self.sync_health(self.player_entity_id, world));

        out
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
    /// Carry out effects produced outside a packet handler, and say what to send.
    ///
    /// The admin path needs this: [`Self::clear_rucksack`] answers with effects rather than
    /// packets, because what it changed is the model's business and what to send is the
    /// session's.
    pub fn apply_effects(&mut self, effects: Vec<Effect>, world: &World) -> Vec<Vec<u8>> {
        self.apply(effects, world)
    }

    fn apply(&mut self, effects: Vec<Effect>, world: &World) -> Vec<Vec<u8>> {
        let out: Vec<Vec<u8>> = effects
            .into_iter()
            .filter_map(|effect| self.packet_for(effect, world))
            .collect();

        // Every route by which a player comes to hold something ends here -- a craft
        // collected, a pickup walked over, a dismantle, an admin `/give` -- so this is the one
        // place a discovery has to be noticed.
        self.announce_discoveries();

        out
    }

    /// "You have never seen one of these before", once per resource.
    ///
    /// Queued as a notification rather than returned: a discovery is something that happens
    /// *to* a player as a side effect of something else, and the packet is a toast the client's
    /// handler drops on its own if it disagrees (`FUN_0073b140` keeps its own seen set).
    fn announce_discoveries(&mut self) {
        if !discovery_toasts_enabled() {
            return;
        }

        let held: Vec<u32> = self
            .inventory()
            .iter()
            .copied()
            .chain(self.drop_slots)
            .filter(|item| *item != 0)
            .filter_map(|item| self.inventories.name(item))
            .collect();

        for resource in held {
            if !self.seen_resources.insert(resource) {
                continue;
            }

            self.notifications.push(encode(|w| {
                NewResourceEncountered {
                    entity_id: self.player_entity_id,
                    item_spec: ItemSpec {
                        resource: Some(resource),
                        ..Default::default()
                    },
                }
                .encode(w)
            }));
        }
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
        let component = self.inventories.item(entity)?;

        // A tool is a different *entity* from a stack of dirt: it carries a durability
        // component, and the repair square refuses anything that does not resolve one.
        if let Some(durability) = self.durability_of(entity, world) {
            if let Some(definition) = world.durable_item_definition() {
                return Some((
                    Entity::new(
                        entity,
                        vec![
                            Component::Durability(durability),
                            Component::InventoryItem(component.clone()),
                        ],
                    ),
                    definition,
                ));
            }
        }

        let definition = world.item_definition().or_else(|| {
            warn!("BasicInventoryItem is not defined; cannot serialise a stack");

            None
        })?;

        Some((
            Entity::new(entity, vec![Component::InventoryItem(component.clone())]),
            definition,
        ))
    }

    /// How worn the stack in `entity` is, or `None` if it is not the kind of thing that wears.
    ///
    /// A stack is a fresh item every time one is minted, so the numbers come straight from the
    /// data: a sword is a sword. Wear that a player has *done* is not modelled yet, which is
    /// the next thing repair needs.
    pub fn durability_of(
        &self,
        entity: u32,
        world: &World,
    ) -> Option<skysaga_world::DurabilityComponent> {
        if !durable_items_enabled() {
            return None;
        }

        let name = self.inventories.name(entity)?;

        world
            .geodata
            .durability_of(name)
            .map(skysaga_world::DurabilityComponent::new)
    }

    /// Create a stack and say what the client must be told, for tests and for probes.
    ///
    /// `give` alone changes the model and sends nothing; this is the pair, which is what the
    /// admin path does.
    pub fn give_announced(&mut self, item: &str, count: u32, world: &World) -> Vec<Vec<u8>> {
        let Some(entity) = self.give(item, count) else {
            return Vec::new();
        };

        self.apply(
            vec![
                Effect::ItemCreated { entity },
                Effect::SlotsChanged {
                    owner: self.player_entity_id,
                },
            ],
            world,
        )
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

