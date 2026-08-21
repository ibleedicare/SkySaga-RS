//! The socket layer: owns the RakNet peer, moves bytes to and from [`crate::Session`].
//!
//! Deliberately thin. Everything that decides anything is in `Session`, which is pure.

use std::collections::HashMap;
use std::net::SocketAddr;

use std::sync::Arc;

use raknet::{message_id, Guid, Peer};
use skysaga_state::{AppState, PlayerSummary, ServerSnapshot, WorldSummary};
use tracing::{info, warn};

use crate::{ClientPacket, Session, World};

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

pub struct GameServer {
    peer: Peer,
    world: World,
    sessions: HashMap<Guid, Session>,

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

        Ok(Self {
            peer,
            world,
            sessions: HashMap::new(),
            state,
        })
    }

    /// Drain every queued packet and answer it.
    ///
    /// **Drains until empty.** The C# takes one packet per 30 ms tick, which caps that whole
    /// server at about 33 packets a second and is the documented cause of its interact lag.
    pub fn tick(&mut self) {
        self.drain();

        // After draining, so a client that connected this tick is already visible.
        self.publish_snapshot();
    }

    fn drain(&mut self) {
        while let Some(packet) = self.peer.receive() {
            let guid = packet.guid();
            let id = packet.message_id();

            match id {
                message_id::NEW_INCOMING_CONNECTION => {
                    info!(guid = guid.0, "client connected");

                    // One player entity per connection. Which entity that is comes from the
                    // world for now; a real registry assigns it per connection.
                    let mut session = Session::new(self.world.player_entity_id);

                    // Character creation ends with the client reconnecting, so by this point
                    // the name and appearance are in storage but have not been sent over
                    // *this* socket. Seeding them is what makes the player arrive in the
                    // world as the character they created rather than as the defaults.
                    session.restore(Self::stored_profile(&self.state));

                    self.sessions.insert(guid, session);
                }

                message_id::DISCONNECTION_NOTIFICATION | message_id::CONNECTION_LOST => {
                    info!(guid = guid.0, "client gone");

                    self.sessions.remove(&guid);
                }

                _ if id >= message_id::ID_USER_PACKET_ENUM => {
                    let Some(session) = self.sessions.get_mut(&guid) else {
                        warn!(guid = guid.0, id, "packet from an unknown connection");
                        continue;
                    };

                    let replies = session.handle(ClientPacket::parse(packet.data()), &self.world);
                    let profile = session.character().clone();

                    // Drop the borrow of `packet` before sending.
                    drop(packet);

                    // Publish what the player told us over RakNet so the HTTP side agrees.
                    Self::publish(&self.state, &profile);

                    for reply in replies {
                        self.peer.send(guid, &reply);
                    }
                }

                _ => {}
            }
        }
    }

    /// The character already in shared state, as a profile to seed a new session with.
    ///
    /// The inverse of [`Self::publish`], and it resolves the account the same way and with
    /// the same limitation: the RakNet connection carries no account, so the most recent
    /// sign-in is used.
    fn stored_profile(state: &AppState) -> crate::CharacterProfile {
        use std::net::{IpAddr, Ipv4Addr};

        let Some(character) = state
            .account_for_peer(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
            .and_then(|account| state.character(&account))
        else {
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
    /// The RakNet connection does not carry an account, so this uses the state's
    /// most-recently-signed-in account -- the same fallback the web endpoints use for a peer
    /// they do not recognise. That is exact for one player and degrades for several, which is
    /// the documented limit of attributing HTTP requests by address.
    fn publish(state: &AppState, profile: &crate::CharacterProfile) {
        use std::net::{IpAddr, Ipv4Addr};

        let Some(account) = state.account_for_peer(IpAddr::V4(Ipv4Addr::UNSPECIFIED)) else {
            return;
        };

        // Make sure a character exists before naming it: the client can finish creation
        // before anything has asked for the character over HTTP.
        if state.character(&account).is_none() {
            let _ = state.ensure_character(&account);
        }

        if let Some(name) = &profile.name {
            let _ = state.set_character_name(&account, name);
        }

        if let Some(biome) = &profile.home_biome {
            let _ = state.set_home_biome(&account, biome);
        }

        if let Some(appearance) = &profile.appearance {
            let _ = state.set_appearance(&account, appearance.clone());
        }
    }

    /// Publish what this server is doing, for anything that wants to look.
    ///
    /// Called at the end of every tick. The world and the sessions live here, on this thread,
    /// and this is how they are made visible without letting anyone reach into them.
    fn publish_snapshot(&self) {
        use std::net::{IpAddr, Ipv4Addr};

        let account = self
            .state
            .account_for_peer(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

        let players = self
            .sessions
            .values()
            .map(|session| {
                let inventory = self
                    .world
                    .player_template
                    .as_ref()
                    .and_then(|(entity, _)| entity.component("clientinventorycomponent"))
                    .and_then(|component| match component {
                        skysaga_world::Component::Inventory(inventory) => Some(inventory),
                        _ => None,
                    });

                PlayerSummary {
                    // The RakNet connection carries no account, so this is the same
                    // most-recent-sign-in fallback the rest of the server uses: exact for one
                    // player, approximate for several.
                    account: account.clone(),
                    character: session.character().name.clone(),
                    entity_id: session.player_entity_id(),
                    stage: format!("{:?}", session.stage()),
                    inventory_slots: inventory.map(|i| i.max_inventory_slots).unwrap_or(0),
                    inventory_items: inventory
                        .map(|i| i.inventory_entity_list.clone())
                        .unwrap_or_default(),
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
