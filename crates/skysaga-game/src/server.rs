//! The socket layer: owns the RakNet peer, moves bytes to and from [`crate::Session`].
//!
//! Deliberately thin. Everything that decides anything is in `Session`, which is pure.

use std::collections::HashMap;
use std::net::SocketAddr;

use raknet::{message_id, Guid, Peer};
use skysaga_proto::client_build::ClientBuild;
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

    /// Which client build this server serves.
    ///
    /// One server serves one build. The two share no packet ids and `NEW_INCOMING_CONNECTION`
    /// carries no build marker, so there is nothing to detect a build *from* — the split is
    /// made earlier, by `game-conductor/retrieve` handing each build a different port.
    pub build: ClientBuild,
}

impl Default for GameServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            max_connections: 32,
            password: DEFAULT_PASSWORD.to_vec(),
            build: ClientBuild::B10414,
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
            build: ClientBuild::from_env(),
            ..default
        }
    }
}

pub struct GameServer {
    peer: Peer,
    world: World,
    sessions: HashMap<Guid, Session>,
    build: ClientBuild,
}

impl GameServer {
    /// Bind the UDP socket and start accepting connections.
    pub fn bind(config: &GameServerConfig, world: World) -> anyhow::Result<Self> {
        let peer = Peer::new();

        peer.set_incoming_password(&config.password);
        peer.set_maximum_incoming_connections(config.max_connections);
        peer.startup(config.port, config.max_connections)?;

        let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

        info!(
            addr = %addr,
            chunks = world.chunks.len(),
            entities = world.entities.len(),
            build = ?config.build,
            "game server listening",
        );

        Ok(Self {
            peer,
            world,
            sessions: HashMap::new(),
            build: config.build,
        })
    }

    /// Drain every queued packet and answer it.
    ///
    /// **Drains until empty.** The C# takes one packet per 30 ms tick, which caps that whole
    /// server at about 33 packets a second and is the documented cause of its interact lag.
    pub fn tick(&mut self) {
        while let Some(packet) = self.peer.receive() {
            let guid = packet.guid();
            let id = packet.message_id();

            match id {
                message_id::NEW_INCOMING_CONNECTION => {
                    info!(guid = guid.0, "client connected");

                    // One player entity per connection. Which entity that is comes from the
                    // world for now; a real registry assigns it per connection.
                    self.sessions.insert(
                        guid,
                        Session::new_for(self.world.player_entity_id, self.build),
                    );
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

                    let client_packet =
                        ClientPacket::from_wire_id_for(u16::from(id), session.build());
                    let replies = session.handle(client_packet, &self.world);

                    // Drop the borrow of `packet` before sending.
                    drop(packet);

                    // An empty reply is a packet this build has no id for; `Session::handle`
                    // has already logged it. Sending it would put a zero-length body on the
                    // wire.
                    for reply in replies.iter().filter(|reply| !reply.is_empty()) {
                        self.peer.send(guid, reply);
                    }
                }

                _ => {}
            }
        }
    }

    pub fn connection_count(&self) -> usize {
        self.sessions.len()
    }
}
