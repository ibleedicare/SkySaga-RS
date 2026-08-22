//! The socket layer: owns the RakNet peer, moves bytes to and from [`crate::Session`].
//!
//! Deliberately thin. Everything that decides anything is in `Session`, which is pure.

use std::collections::HashMap;
use std::net::SocketAddr;

use std::sync::Arc;

use raknet::{message_id, Guid, Peer};
use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::{Bits, EntityAdd, EntityRemoved, EntitySync};
use skysaga_world::{Component, Entity};
use skysaga_state::{AdminCommand, AppState, PlayerSummary, ServerSnapshot, WorldSummary};
use tracing::{info, warn};

use crate::{encode, ClientPacket, Session, World};

/// The port the client is told to connect to by `game-conductor/retrieve`.
pub const DEFAULT_PORT: u16 = 42069;

/// The password the client presents. From `SkySaga.Game/Program.cs`; the trailing NUL is part
/// of it, because the C# passes `"...\0"` with `password.Length`.
pub const DEFAULT_PASSWORD: &[u8] = b"Something about penguins\0";

#[derive(Debug, Clone)]
pub struct GameServerConfig {
    pub port: u16,
    pub max_connections: u16,
    pub password: Vec<u8>,
}

impl Default for GameServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            max_connections: 32,
            password: DEFAULT_PASSWORD.to_vec(),
        }
    }
}

impl GameServerConfig {
    pub fn from_env() -> Self {
        let default = Self::default();

        Self {
            port: std::env::var("SKYSAGA_GAME_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(default.port),
            ..default
        }
    }
}

/// Ordinals the server relays between players rather than acting on.
mod relayed {
    /// `EntityMoved`: where a player is now.
    pub const ENTITY_MOVED: u16 = 102;
    /// `SetPlayerState`: standing, jumping, and so on.
    pub const SET_PLAYER_STATE: u16 = 35;
}

pub struct GameServer {
    peer: Peer,
    world: World,
    sessions: HashMap<Guid, Session>,

    /// Each connection's player body, by connection.
    ///
    /// The world holds one player entity built at startup; that is now a template. A body per
    /// connection is what lets two players exist at once, and mirrors the C#, which creates a
    /// `Player` entity in its map per connection.
    bodies: HashMap<Guid, EntityAdd>,

    /// The next free entity id.
    ///
    /// Starts past everything the world was built with, so a player body can never collide
    /// with a prop. Never reused: an id handed out twice would have the client destroy the
    /// entity it already holds under that id.
    next_entity_id: u32,

    /// Shared with the web and auth servers.
    ///
    /// Character creation happens over RakNet, but `characters/list` is answered over HTTP.
    /// Without one state between them the client finishes creating a character, the web layer
    /// still reports it as incomplete, and the client loops back into the creator -- observed
    /// exactly that when the two ran as separate processes.
    state: Arc<AppState>,
}

impl GameServer {
    /// Bind the UDP socket and start accepting connections.
    pub fn bind(
        config: &GameServerConfig,
        world: World,
        state: Arc<AppState>,
    ) -> anyhow::Result<Self> {
        let peer = Peer::new();

        peer.set_incoming_password(&config.password);
        peer.set_maximum_incoming_connections(config.max_connections);
        peer.startup(config.port, config.max_connections)?;

        let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

        info!(
            addr = %addr,
            chunks = world.chunks.len(),
            entities = world.entities.len(),
            "game server listening",
        );

        let next_entity_id = world
            .entities
            .iter()
            .map(|entity| entity.id)
            .max()
            .unwrap_or(0)
            + 1;

        Ok(Self {
            peer,
            world,
            sessions: HashMap::new(),
            bodies: HashMap::new(),
            next_entity_id,
            state,
        })
    }

    /// Drain every queued packet and answer it.
    ///
    /// **Drains until empty.** The C# takes one packet per 30 ms tick, which caps that whole
    /// server at about 33 packets a second and is the documented cause of its interact lag.
    pub fn tick(&mut self) {
        self.drain();

        // Admin requests, which may only be carried out here: the world is this thread's.
        for command in self.state.take_commands() {
            self.apply(command);
        }

        // After draining, so a client that connected this tick is already visible.
        self.publish_snapshot();
    }

    fn drain(&mut self) {
        while let Some(packet) = self.peer.receive() {
            let guid = packet.guid();
            let id = packet.message_id();

            match id {
                message_id::NEW_INCOMING_CONNECTION => {
                    let entity_id = self.next_entity_id;
                    self.next_entity_id += 1;

                    info!(guid = guid.0, entity = entity_id, "client connected");

                    let mut session = Session::new(entity_id);

                    // Who this connection belongs to, from the conductor's reservation. The
                    // connection carries no account of its own.
                    let account = self.state.claim_slot();

                    if account.is_none() {
                        warn!(
                            guid = guid.0,
                            "no reservation for this connection; it will have no account",
                        );
                    }

                    session.set_account(account.clone());

                    // Character creation ends with the client reconnecting, so by this point
                    // the name and appearance are in storage but have not been sent over
                    // *this* socket. Seeding them is what makes the player arrive in the
                    // world as the character they created rather than as the defaults.
                    session.restore(Self::stored_profile(&self.state, account.as_deref()));

                    let body = self.world.player_body(session.character(), entity_id, &[]);

                    self.sessions.insert(guid, session);
                    self.bodies.insert(guid, body.clone());

                    // Tell everyone already here that someone arrived. The C# does not do
                    // this -- its OnConnected is empty -- which is why a second player is
                    // invisible to the first there even though the reverse works.
                    self.announce(guid, &encode(|w| body.encode(w)));
                }

                message_id::DISCONNECTION_NOTIFICATION | message_id::CONNECTION_LOST => {
                    info!(guid = guid.0, "client gone");

                    self.sessions.remove(&guid);

                    // Without this the departed player stands motionless in everyone else's
                    // world until they restart the client.
                    if let Some(body) = self.bodies.remove(&guid) {
                        let removed = encode(|w| {
                            EntityRemoved {
                                entity_id: body.id,
                            }
                            .encode(w)
                        });

                        self.announce(guid, &removed);
                    }
                }

                _ if id >= message_id::ID_USER_PACKET_ENUM => {
                    if !self.sessions.contains_key(&guid) {
                        warn!(guid = guid.0, id, "packet from an unknown connection");
                        continue;
                    }

                    let data = packet.data().to_vec();

                    // Drop the borrow of `packet` before touching anything else.
                    drop(packet);

                    let others: Vec<EntityAdd> = self
                        .bodies
                        .iter()
                        .filter(|(other, _)| **other != guid)
                        .map(|(_, body)| body.clone())
                        .collect();

                    let Some(session) = self.sessions.get_mut(&guid) else {
                        continue;
                    };

                    let incoming = ClientPacket::parse(&data);

                    // Creating a homeworld is answered with TransferToServer, which sends the
                    // client straight back to us. It reconnects without asking the conductor,
                    // so nothing would reserve a slot for it and the new connection would
                    // arrive with no account: the player would be handed a default character
                    // and lose the appearance they had just chosen.
                    let transferring = matches!(incoming, ClientPacket::CreateHomeworld(_));

                    // A packet may mint entities -- splitting a stack does -- and ids are
                    // allocated globally so one connection's split cannot collide with
                    // another's body. Hand the session the allocator, then take it back.
                    session.reserve_ids_from(self.next_entity_id);

                    let replies = session.handle_with(incoming, &self.world, &others);

                    self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

                    let profile = session.character().clone();
                    let account = session.account().map(str::to_owned);
                    let inventory = session.inventory().to_vec();

                    if transferring {
                        if let Some(account) = &account {
                            self.state.reserve_slot(account);
                        }
                    }

                    // The body other players see is built from the profile, and the profile
                    // has just changed. Rebuilding it here keeps them from seeing whatever
                    // this player looked like when they first connected.
                    if let Some(body) = self.bodies.get_mut(&guid) {
                        *body = self.world.player_body(&profile, body.id, &inventory);
                    }

                    // Movement is not interpreted, only passed on: the sender has already
                    // decided where they are, and the others need to be told. Relaying the
                    // bytes unchanged means no decoder to get wrong.
                    let ordinal = id as u16 - message_id::ID_USER_PACKET_ENUM as u16;

                    if matches!(ordinal, relayed::ENTITY_MOVED | relayed::SET_PLAYER_STATE) {
                        self.announce(guid, &data);
                    }

                    // Publish what the player told us over RakNet so the HTTP side agrees.
                    Self::publish(&self.state, account.as_deref(), &profile);

                    for reply in replies {
                        self.peer.send(guid, &reply);
                    }

                    // Anything the session wants to send that this packet did not ask for.
                    //
                    // Inlined rather than a call to `flush_notifications`: the receive loop
                    // holds a borrow of `self.peer`, so a `&mut self` method cannot be called
                    // from inside it. `sessions` and `peer` are disjoint fields, which is what
                    // makes this form legal where the method call is not.
                    if let Some(session) = self.sessions.get_mut(&guid) {
                        for packet in session.take_notifications() {
                            self.peer.send(guid, &packet);
                        }
                    }
                }

                _ => {}
            }
        }
    }

    /// The character already in shared state, as a profile to seed a new session with.
    ///
    /// The inverse of [`Self::publish`]. `account` comes from the conductor's reservation;
    /// with none, the session starts with no character and the client runs its creator.
    fn stored_profile(state: &AppState, account: Option<&str>) -> crate::CharacterProfile {
        let Some(character) = account.and_then(|account| state.character(account)) else {
            return crate::CharacterProfile::default();
        };

        crate::CharacterProfile {
            // A character that has never been through the creator carries the account name
            // as a placeholder, which is still the right thing to show above its head.
            name: Some(character.name),
            home_biome: character.home_biome,
            appearance: Some(character.appearance),
        }
    }

    /// Push a session's character profile into the shared state.
    ///
    /// `account` comes from the conductor's reservation, claimed when the connection arrived.
    fn publish(state: &AppState, account: Option<&str>, profile: &crate::CharacterProfile) {
        // Without an account there is nowhere to put this. It would previously have been
        // written to whoever signed in last, which is how one player's character ended up
        // under another player's name.
        let Some(account) = account else {
            return;
        };

        // Make sure a character exists before naming it: the client can finish creation
        // before anything has asked for the character over HTTP.
        if state.character(account).is_none() {
            let _ = state.ensure_character(account);
        }

        if let Some(name) = &profile.name {
            let _ = state.set_character_name(account, name);
        }

        if let Some(biome) = &profile.home_biome {
            let _ = state.set_home_biome(account, biome);
        }

        if let Some(appearance) = &profile.appearance {
            let _ = state.set_appearance(account, appearance.clone());
        }
    }

    /// Carry out an admin request.
    fn apply(&mut self, command: AdminCommand) {
        match command {
            AdminCommand::Give {
                account,
                item,
                count,
            } => self.give(&account, &item, count),

            AdminCommand::Mail {
                account,
                subject,
                body,
                attachments,
            } => self.mail(&account, &subject, &body, &attachments),

            AdminCommand::Chest {
                account,
                entity,
                loot,
            } => self.chest(&account, &entity, &loot),
        }
    }

    /// Put a chest in the world, in front of a player.
    ///
    /// Announced only to that player. The world is fixed once built, so a spawned container
    /// lives on the session and nobody else's session knows about it -- the same limitation as
    /// the containers the world seeds, and it lifts at the same time.
    fn chest(&mut self, account: &str, entity: &str, loot: &[String]) {
        let Some(guid) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, _)| *guid)
        else {
            warn!(%account, "cannot spawn a chest: that player is not connected");

            return;
        };

        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        session.reserve_ids_from(self.next_entity_id);

        let borrowed: Vec<&str> = loot.iter().map(String::as_str).collect();

        let Some(spawned) = session.spawn_chest(&self.world, entity, &borrowed) else {
            warn!(%account, %entity, "no such entity in Entities.json");

            return;
        };

        self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

        // In the order the session gave them: the loot, then the chest that names it.
        for packet in &spawned.packets {
            self.peer.send(guid, packet);
        }

        info!(
            %account,
            %entity,
            id = spawned.entity,
            position = ?spawned.position,
            "spawned a chest",
        );
    }

    /// Put a stack of `item` into a player's rucksack.
    ///
    /// Follows the C#: create an entity for the stack, announce it, then point a free slot at
    /// its id. The order matters -- the client must know the entity before a slot references
    /// it, or the slot points at nothing.
    ///
    /// The item name is hashed rather than looked up. `geodata.json` holds the resource table
    /// and this server does not read it yet, so an unknown name produces a stack the client
    /// cannot draw instead of an error here. That is the one thing this is worse at than the
    /// C#, which refuses `unknown item 'x'`.
    fn give(&mut self, account: &str, item: &str, count: u32) {
        let Some((guid, entity_id)) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, session)| (*guid, session.player_entity_id()))
        else {
            warn!(%account, "cannot give: that player is not connected");

            return;
        };

        let Some(definition) = self.world.item_definition().cloned() else {
            warn!("cannot give: BasicInventoryItem is not defined");

            return;
        };

        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        // Ids come from the server's allocator, so a stack can never collide with another
        // connection's body.
        session.reserve_ids_from(self.next_entity_id);

        let Some(item_entity) = session.give(item, count) else {
            warn!(%account, "cannot give: the rucksack is full");

            return;
        };

        self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

        let slot = session.slot_of(item_entity).unwrap_or_default();

        // The client must know the entity before a slot points at it, or the slot references
        // an entity it has never been told about and the square draws empty.
        if let Some(component) = session.inventories().item(item_entity) {
            let stack = Entity::new(
                item_entity,
                vec![Component::InventoryItem(component.clone())],
            );

            self.peer
                .send(guid, &encode(|w| stack.to_entity_add(&definition).encode(w)));
        }

        let Some(session) = self.sessions.get(&guid) else {
            return;
        };

        let profile = session.character().clone();
        let inventory = session.inventory().to_vec();

        // EntitySync, never a repeat EntityAdd. A second EntityAdd for an id the client holds
        // makes it destroy the entity and build a fresh one, leaving every slot list that
        // still names the old object holding a dangling pointer.
        if let Some((entity, definition)) =
            self.world.player_entity(&profile, entity_id, &inventory)
        {
            let mut payload = BitWriter::new();

            // Only the inventory. The C# syncs what changed and nothing else, and the
            // client is never sent a full update for an entity it already holds.
            entity
                .sync_data_for(definition, &["inventoryentitylist"])
                .encode(&mut payload);

            let sync = EntitySync {
                id: entity_id,
                sync_data: Bits::from_writer(&payload),
            };

            self.peer.send(guid, &encode(|w| sync.encode(w)));
        }

        info!(%account, %item, count, slot, entity = item_entity, "gave an item");
    }

    /// Send whatever a session has queued that no client packet asked for.
    ///
    /// The mail doorbell, so far. `Session::handle` answers a request; this is how something
    /// that happens *to* a player -- a message arriving while the panel is shut -- reaches
    /// them.
    fn flush_notifications(&mut self, guid: Guid) {
        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        for packet in session.take_notifications() {
            self.peer.send(guid, &packet);
        }
    }

    /// Put a message in a player's inbox.
    ///
    /// The attachment container is announced **once, here**, before anything references it. A
    /// repeat `EntityAdd` for an id the client already holds makes it destroy the entity and
    /// build a fresh one, and every slot list still naming the old object is then holding a
    /// dangling pointer -- which is the one shape that faults the client's contents recompute.
    /// Later changes ride `EntitySync`, which is the path `give` already proves.
    fn mail(&mut self, account: &str, subject: &str, body: &str, attachments: &[(String, u32)]) {
        let Some(guid) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, _)| *guid)
        else {
            warn!(%account, "cannot send mail: that player is not connected");

            return;
        };

        let Some(definition) = self.world.item_definition().cloned() else {
            warn!("cannot send mail: BasicInventoryItem is not defined");

            return;
        };

        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        session.reserve_ids_from(self.next_entity_id);

        let borrowed: Vec<(&str, u32)> = attachments
            .iter()
            .map(|(name, count)| (name.as_str(), *count))
            .collect();

        let uuid = session.compose(subject, body, &borrowed);

        self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

        // Announce the attachment items themselves, so the container's slot list names
        // entities the client has been told about.
        let Some(mail) = session.mail(&uuid).cloned() else {
            return;
        };

        let items: Vec<(u32, skysaga_world::InventoryItemComponent)> = session
            .inventories()
            .slots(mail.attachment_entity)
            .iter()
            .copied()
            .filter(|item| *item != 0)
            .filter_map(|item| {
                session
                    .inventories()
                    .item(item)
                    .map(|component| (item, component.clone()))
            })
            .collect();

        for (id, component) in items {
            let stack = Entity::new(id, vec![Component::InventoryItem(component)]);

            self.peer
                .send(guid, &encode(|w| stack.to_entity_add(&definition).encode(w)));
        }

        self.flush_notifications(guid);

        info!(%account, subject, uuid = %mail.uuid, "sent mail");
    }

    /// Send to every connection except `from`.
    ///
    /// Used for the things other players need to know: someone arrived, someone left, someone
    /// moved. The sender is skipped because it already knows.
    fn announce(&self, from: Guid, data: &[u8]) {
        for guid in self.sessions.keys() {
            if *guid != from {
                self.peer.send(*guid, data);
            }
        }
    }

    /// Publish what this server is doing, for anything that wants to look.
    ///
    /// Called at the end of every tick. The world and the sessions live here, on this thread,
    /// and this is how they are made visible without letting anyone reach into them.
    fn publish_snapshot(&self) {
        let players = self
            .sessions
            .values()
            .map(|session| {
                // How many slots the rucksack has comes from the template, since every player
                // starts with the same one. What is *in* it comes from the session: items are
                // given while the session runs, and the template never changes.
                let slots = self
                    .world
                    .player_template
                    .as_ref()
                    .and_then(|(entity, _)| entity.component("clientinventorycomponent"))
                    .and_then(|component| match component {
                        Component::Inventory(inventory) => Some(inventory.max_inventory_slots),
                        _ => None,
                    })
                    .unwrap_or(0);

                PlayerSummary {
                    account: session.account().map(str::to_owned),
                    character: session.character().name.clone(),
                    entity_id: session.player_entity_id(),
                    stage: format!("{:?}", session.stage()),
                    inventory_slots: slots,
                    inventory_items: session.inventory().to_vec(),
                }
            })
            .collect();

        self.state.publish_snapshot(ServerSnapshot {
            world: WorldSummary {
                adventure: self.world.adventure.clone(),
                biome: self.world.server_info.biome.clone(),
                chunks: self.world.chunks.len(),
                entities: self.world.entities.len(),
            },
            players,
        });
    }

    pub fn connection_count(&self) -> usize {
        self.sessions.len()
    }
}
