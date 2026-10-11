//! The socket layer: owns the RakNet peer, moves bytes to and from [`crate::Session`].
//!
//! Deliberately thin. Everything that decides anything is in `Session`, which is pure.

use std::collections::HashMap;
use std::net::SocketAddr;

use std::sync::Arc;

use raknet::{message_id, Guid, Peer};
use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::{Bits, EntityAdd, EntityRemoved, EntitySync};
use skysaga_world::Component;
use skysaga_state::{AdminCommand, AppState, PlayerSummary, ServerSnapshot, WorldSummary};
use tracing::{info, warn};

use crate::{encode, ClientPacket, Destination, Session, World};

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

/// Everyone in the same world as `from`, without `from`.
///
/// One process hosts several worlds, and a player must only hear about the people standing
/// in theirs: an arrival, a step or a swing sent across worlds draws a body where nobody is.
fn neighbours(sessions: &HashMap<Guid, Session>, from: Guid) -> Vec<Guid> {
    let Some(world) = sessions.get(&from).map(Session::world_key) else {
        return Vec::new();
    };

    sessions
        .iter()
        .filter(|(guid, session)| **guid != from && session.world_key() == world)
        .map(|(guid, _)| *guid)
        .collect()
}

/// The world a connection is in: the one its key names, or home.
///
/// A function over the two fields rather than a method, so it can be called while `sessions`
/// is borrowed mutably.
fn world_of<'a>(
    home: &'a World,
    adventures: &'a HashMap<String, World>,
    key: Option<&str>,
) -> &'a World {
    key.and_then(|key| adventures.get(key)).unwrap_or(home)
}

pub struct GameServer {
    peer: Peer,
    world: World,

    /// The worlds portals lead to, by [`Destination::key`], made the first time one is needed.
    ///
    /// One process hosts them all: a player going through a portal is sent back to this same
    /// address and finds a different world there.
    adventures: HashMap<String, World>,

    /// Where each account is headed, for the connection it is about to make.
    ///
    /// A connection carries no account and no destination; both are joined to it when it
    /// arrives. An account not listed here goes home.
    destinations: HashMap<String, Destination>,

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

        // Past the props *and* past anything restored on top of them: a restored anvil holds
        // an id, and handing it out again would have the client destroy the anvil and build
        // something else under the same number.
        let next_entity_id = world.next_entity_id();

        Ok(Self {
            peer,
            world,
            adventures: HashMap::new(),
            destinations: HashMap::new(),
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

        // **Every tick, not only after a packet arrives.**
        //
        // A session queues packets nobody asked for -- the mail doorbell, and a craft
        // announcing that its timer has run out -- and draining them only inside `drain` ties
        // them to the client saying something first. A player standing still with a crafting
        // panel open sends *nothing*: no movement, no state, no query. Their craft finished,
        // the announcement sat in the queue, and the panel waited for a packet that could only
        // be sent once the panel let go. Observed live, on a camp fire.
        for (guid, packets) in self
            .sessions
            .iter_mut()
            .map(|(guid, session)| (*guid, session.take_notifications()))
            .filter(|(_, packets)| !packets.is_empty())
            .collect::<Vec<_>>()
        {
            for packet in packets {
                self.peer.send(guid, &packet);
            }
        }

        self.restore_inventories();
        self.restore_hotbars();
        self.restore_mailboxes();
        self.record_inventories();
        self.record_hotbars();
        self.record_mailboxes();
        self.record_blocks();
        self.record_devices();

        // After draining, so a client that connected this tick is already visible.
        self.publish_snapshot();
    }

    /// Give a player back what they were carrying when they last played.
    ///
    /// Once the session is in the world, not during the handshake: the entity burst carries no
    /// item entities, so the stacks have to be created and announced afterwards. See
    /// [`Session::restore_items`].
    fn restore_inventories(&mut self) {
        let restores: Vec<(Guid, Vec<skysaga_state::StoredItem>)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.stage() == crate::Stage::Playing)
            .filter(|(_, session)| !session.items_restored())
            .filter_map(|(guid, session)| {
                let account = session.account()?;

                Some((*guid, self.state.inventory(account)))
            })
            .collect();

        for (guid, items) in restores {
            let Some(session) = self.sessions.get_mut(&guid) else {
                continue;
            };

            for packet in session.restore_items(&items, &self.world) {
                self.peer.send(guid, &packet);
            }

            session.mark_items_restored();
        }
    }

    /// Give a player back the hotbar they had when they last played.
    ///
    /// **After the rucksack**, and only once it is back: a binding is pointed at a stack the
    /// player carries, and those stacks are what `restore_inventories` has just created. It
    /// runs in the same tick, one call later.
    fn restore_hotbars(&mut self) {
        let restores: Vec<(Guid, Vec<skysaga_state::StoredBinding>)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.items_restored() && !session.bindings_restored())
            // Off, the client keeps its own bar: nothing is restored, and because the session
            // is never marked restored, nothing is written over the saved one either.
            .filter(|(_, session)| session.carries_hotbar(&self.world))
            .filter_map(|(guid, session)| {
                let account = session.account()?;

                Some((*guid, self.state.hotbar(account)))
            })
            .collect();

        for (guid, bindings) in restores {
            let Some(session) = self.sessions.get_mut(&guid) else {
                continue;
            };

            for packet in session.restore_bindings(&bindings, &self.world) {
                self.peer.send(guid, &packet);
            }

            session.mark_bindings_restored();
        }
    }

    /// Give a player back the inbox they had when they last played.
    ///
    /// After the handshake, exactly as the rucksack is: a restored message's attachments are
    /// fresh item entities and the client has to be told about them, which cannot happen inside
    /// the entity burst.
    fn restore_mailboxes(&mut self) {
        let restores: Vec<(Guid, Vec<skysaga_state::StoredMail>)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.stage() == crate::Stage::Playing)
            .filter(|(_, session)| !session.mail_restored())
            .filter_map(|(guid, session)| {
                let account = session.account()?;

                Some((*guid, self.state.mail(account)))
            })
            .collect();

        for (guid, mail) in restores {
            let Some(session) = self.sessions.get_mut(&guid) else {
                continue;
            };

            session.reserve_ids_from(self.next_entity_id);

            for packet in session.restore_mail(&mail, &self.world) {
                self.peer.send(guid, &packet);
            }

            self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

            session.mark_mail_restored();
        }
    }

    /// Write down each player's inbox, when it changes.
    fn record_mailboxes(&mut self) {
        let changed: Vec<(String, Vec<skysaga_state::StoredMail>)> = self
            .sessions
            .values_mut()
            .filter_map(|session| {
                let mail = session.mail_to_record()?;
                let account = session.account()?.to_owned();

                Some((account, mail))
            })
            .collect();

        for (account, mail) in changed {
            self.state.set_mail(&account, mail);
        }
    }

    /// Write down the blocks players have changed since the last tick.
    ///
    /// The world queues them rather than reaching the store itself: it is a value on this
    /// thread and persistence lives at the edge, so the queue is drained here exactly as a
    /// session's notifications are.
    fn record_blocks(&mut self) {
        for edit in self.world.take_unsaved_edits() {
            self.state.set_block(skysaga_state::StoredBlock {
                chunk: edit.chunk,
                voxel: edit.voxel,
                material: edit.material,
            });
        }
    }

    /// Write down the devices players have put down since the last tick.
    ///
    /// The same shape as [`Self::record_blocks`], and drained from the world for the same
    /// reason: the world is a value on this thread and persistence lives at the edge.
    fn record_devices(&mut self) {
        for device in self.world.take_unsaved_devices() {
            self.state.set_device(skysaga_state::StoredDevice {
                name: device.name,
                position: device.position,
            });
        }
    }

    /// Write down what each player is carrying, when it changes.
    ///
    /// The comparison is the session's own, so a tick where nothing moved costs one walk of 45
    /// squares and takes no lock at all. Only a real change reaches `AppState`, and from there
    /// the background writer.
    fn record_inventories(&mut self) {
        let changed: Vec<(String, Vec<skysaga_state::StoredItem>)> = self
            .sessions
            .values_mut()
            .filter_map(|session| {
                let items = session.items_to_record()?;

                Some((session.account()?.to_owned(), items))
            })
            .collect();

        for (account, items) in changed {
            self.state.set_inventory(&account, items);
        }
    }

    /// Write down what each player has bound to the hotbar, when it changes.
    fn record_hotbars(&mut self) {
        let changed: Vec<(String, Vec<skysaga_state::StoredBinding>)> = self
            .sessions
            .values_mut()
            .filter_map(|session| {
                let bindings = session.bindings_to_record()?;

                Some((session.account()?.to_owned(), bindings))
            })
            .collect();

        for (account, bindings) in changed {
            self.state.set_hotbar(&account, bindings);
        }
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

                    // Which world: wherever this account set off for, or home.
                    let destination = account
                        .as_deref()
                        .and_then(|account| self.destinations.remove(account));

                    if let Some(Destination::Adventure { biome, adventure }) = &destination {
                        let key = destination.as_ref().and_then(Destination::key).unwrap_or_default();

                        if !self.adventures.contains_key(&key) {
                            info!(%key, "opening a world");

                            self.adventures.insert(
                                key.clone(),
                                self.world.for_adventure(adventure.as_deref(), biome),
                            );
                        }

                        info!(guid = guid.0, %key, "arriving through a portal");

                        session.set_world_key(Some(key));
                    }

                    let body = world_of(&self.world, &self.adventures, session.world_key())
                        .player_body(session.character(), entity_id, &[]);

                    self.sessions.insert(guid, session);
                    self.bodies.insert(guid, body.clone());

                    // Tell everyone already here that someone arrived. The C# does not do
                    // this -- its OnConnected is empty -- which is why a second player is
                    // invisible to the first there even though the reverse works.
                    self.announce(guid, &encode(|w| body.encode(w)));
                }

                message_id::DISCONNECTION_NOTIFICATION | message_id::CONNECTION_LOST => {
                    info!(guid = guid.0, "client gone");

                    // Asked before the session goes: afterwards nothing says which world it
                    // was in.
                    let left_behind = neighbours(&self.sessions, guid);

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

                        for other in left_behind {
                            self.peer.send(other, &removed);
                        }
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

                    let others: Vec<EntityAdd> = neighbours(&self.sessions, guid)
                        .iter()
                        .filter_map(|other| self.bodies.get(other).cloned())
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

                    let world = world_of(&self.world, &self.adventures, session.world_key());

                    let replies = session.handle_with(incoming, world, &others);

                    // A player who has just set off reconnects without asking the conductor,
                    // exactly as after creating a homeworld: the slot is reserved here, and
                    // the destination is kept for the connection that claims it.
                    let departure = session.take_departure();

                    self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

                    let profile = session.character().clone();
                    let account = session.account().map(str::to_owned);
                    let inventory = session.inventory().to_vec();

                    if transferring {
                        if let Some(account) = &account {
                            self.state.reserve_slot(account);
                        }
                    }

                    if let (Some(destination), Some(account)) = (departure, &account) {
                        info!(%account, ?destination, "setting off");

                        self.state.reserve_slot(account);
                        self.destinations.insert(account.clone(), destination);
                    }

                    // The body other players see is built from the profile, and the profile
                    // has just changed. Rebuilding it here keeps them from seeing whatever
                    // this player looked like when they first connected.
                    if let Some(body) = self.bodies.get_mut(&guid) {
                        *body = world.player_body(&profile, body.id, &inventory);
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

                    // ...and anything addressed to everyone *else*: the swing echoes, which
                    // are what make another player's sword move. Same inlining, same reason.
                    let broadcasts = self
                        .sessions
                        .get_mut(&guid)
                        .map(Session::take_broadcasts)
                        .unwrap_or_default();

                    let nearby = neighbours(&self.sessions, guid);

                    for packet in broadcasts {
                        for other in &nearby {
                            self.peer.send(*other, &packet);
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
                durability,
            } => self.give(&account, &item, count, durability),

            AdminCommand::ClearInventory { account } => self.clear_inventory(&account),

            AdminCommand::Durability { enabled } => {
                if let Some(enabled) = enabled {
                    crate::set_durable_items(enabled);
                }

                info!(?enabled, "durable items switched");
            }

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

            AdminCommand::Mob {
                account,
                entity,
                count,
            } => self.mob(&account, &entity, count),

            AdminCommand::Lid {
                account,
                raise_on_close,
            } => {
                let Some(session) = self
                    .sessions
                    .values_mut()
                    .find(|session| session.account() == Some(account.as_str()))
                else {
                    warn!(%account, "cannot set the lid mode: that player is not connected");

                    return;
                };

                session.set_raise_lid_on_close(raise_on_close);

                info!(%account, raise_on_close, "lid mode");
            }

            AdminCommand::Portal {
                account,
                state,
                biome,
                adventure,
            } => {
                let Some((guid, session)) = self
                    .sessions
                    .iter_mut()
                    .find(|(_, session)| session.account() == Some(account.as_str()))
                else {
                    warn!(%account, "cannot set a portal: that player is not connected");

                    return;
                };

                let packets = session.set_last_portal(
                    state,
                    biome.as_deref().map(skysaga_core::name_hash),
                    adventure.as_deref().map(skysaga_core::name_hash),
                    &self.world,
                );

                info!(%account, state, ?biome, ?adventure, packets = packets.len(), "portal");

                for packet in packets {
                    self.peer.send(*guid, &packet);
                }
            }
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

    /// Put creatures in the world, in front of a player.
    ///
    /// Announced only to that player, as a spawned chest is: the world is fixed once built, so
    /// anything created after it lives on the session. Every one lands at the same spot --
    /// they have no AI to walk them apart, so a stack of three is three overlapping bodies
    /// that take three separate fights to clear.
    fn mob(&mut self, account: &str, entity: &str, count: u32) {
        let Some(guid) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, _)| *guid)
        else {
            warn!(%account, "cannot spawn a creature: that player is not connected");

            return;
        };

        for _ in 0..count {
            let Some(session) = self.sessions.get_mut(&guid) else {
                return;
            };

            session.reserve_ids_from(self.next_entity_id);

            let Some(spawned) = session.spawn_creature(&self.world, entity) else {
                warn!(%account, %entity, "not an entity with physical properties");

                return;
            };

            self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

            for packet in &spawned.packets {
                self.peer.send(guid, packet);
            }

            info!(
                %account,
                %entity,
                id = spawned.entity,
                position = ?spawned.position,
                "spawned a creature",
            );
        }
    }

    /// Empty a player's rucksack and tell their client.
    fn clear_inventory(&mut self, account: &str) {
        let Some(guid) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, _)| *guid)
        else {
            warn!(%account, "cannot clear: that player is not connected");

            return;
        };

        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        let effects = session.clear_rucksack();

        if effects.is_empty() {
            return;
        }

        info!(%account, "cleared the rucksack");

        for packet in session.apply_effects(effects, &self.world) {
            self.peer.send(guid, &packet);
        }
    }

    /// Put a stack of `item` into a player's rucksack.
    ///
    /// Follows the C#: create an entity for the stack, announce it, then point a free slot at
    /// its id. The order matters -- the client must know the entity before a slot references
    /// it, or the slot points at nothing.
    ///
    /// **The name is checked before anything is minted.** A hash of a misspelling is a
    /// perfectly good number that resolves to no resource, so an unchecked `/give` fills a
    /// square with a stack the client cannot draw and reports success everywhere the server
    /// looks. See [`Session::give_checked`].
    fn give(&mut self, account: &str, item: &str, count: u32, durability: Option<u32>) {
        let Some((guid, entity_id)) = self
            .sessions
            .iter()
            .find(|(_, session)| session.account() == Some(account))
            .map(|(guid, session)| (*guid, session.player_entity_id()))
        else {
            warn!(%account, "cannot give: that player is not connected");

            return;
        };

        let Some(session) = self.sessions.get_mut(&guid) else {
            return;
        };

        // Ids come from the server's allocator, so a stack can never collide with another
        // connection's body.
        session.reserve_ids_from(self.next_entity_id);

        let item_entity = match session.give_checked(item, count, &self.world) {
            Ok(entity) => entity,

            Err(refusal) => {
                warn!(%account, %item, "cannot give: {refusal}");

                return;
            }
        };

        self.next_entity_id = self.next_entity_id.max(session.next_entity_id());

        // Before it is announced, so the client is told once and told the worn number.
        if let Some(left) = durability {
            session.wear_to(item_entity, left, &self.world);
        }

        let slot = session.slot_of(item_entity).unwrap_or_default();

        // The client must know the entity before a slot points at it, or the slot references
        // an entity it has never been told about and the square draws empty.
        //
        // Through the session, which knows a tool from a stack of dirt: building the packet
        // here from the basic definition sent every sword as a `BasicInventoryItem`.
        for packet in session.announce_item(item_entity, &self.world) {
            self.peer.send(guid, &packet);
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

        let Some(mail) = session.mail(&uuid).cloned() else {
            return;
        };

        // The attachment items, and **the container that names them**. One helper for both this
        // path and the restore, because getting the order wrong -- or leaving the container out
        // -- is a message that arrives with no attachments, and the two paths would otherwise
        // have to be got right twice.
        let announcements = session.announce_attachments(&uuid, &self.world);

        for packet in announcements {
            self.peer.send(guid, &packet);
        }

        self.flush_notifications(guid);

        info!(%account, subject, uuid = %mail.uuid, "sent mail");
    }

    /// Send to everyone in `from`'s world except `from`.
    ///
    /// Used for the things other players need to know: someone arrived, someone moved. The
    /// sender is skipped because it already knows.
    fn announce(&self, from: Guid, data: &[u8]) {
        for guid in neighbours(&self.sessions, from) {
            self.peer.send(guid, data);
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
                    position: session.position(),
                    facing_yaw: session.facing_yaw(),
                    using_entity_id: session.using_entity(),
                }
            })
            .collect();

        self.state.publish_snapshot(ServerSnapshot {
            world: WorldSummary {
                adventure: self.world.adventure.clone(),
                biome: self.world.server_info.biome.clone(),
                chunks: self.world.chunks.len(),
                entities: self.world.entities.len(),
                spawn_voxel: self.world.spawn_voxel,
            },
            players,
        });
    }

    pub fn connection_count(&self) -> usize {
        self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_in(world: Option<&str>) -> Session {
        let mut session = Session::new(1);

        session.set_world_key(world.map(str::to_owned));

        session
    }

    #[test]
    fn neighbours_are_the_players_in_the_same_world() {
        let sessions = HashMap::from([
            (Guid(1), session_in(None)),
            (Guid(2), session_in(None)),
            (Guid(3), session_in(Some("Forest/Castle_Adventure"))),
            (Guid(4), session_in(Some("Forest/Castle_Adventure"))),
            (Guid(5), session_in(Some("Desert"))),
        ]);

        assert_eq!(neighbours(&sessions, Guid(1)), [Guid(2)]);
        assert_eq!(neighbours(&sessions, Guid(3)), [Guid(4)]);
        assert!(neighbours(&sessions, Guid(5)).is_empty());
    }

    #[test]
    fn a_connection_nobody_knows_has_no_neighbours() {
        let sessions = HashMap::from([(Guid(1), session_in(None))]);

        assert!(neighbours(&sessions, Guid(9)).is_empty());
    }
}
