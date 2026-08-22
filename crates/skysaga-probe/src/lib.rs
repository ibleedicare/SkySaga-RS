//! A headless client: connects, plays the handshake, and reports what it was told.
//!
//! The real client is a 3D game under Wine. Starting two of them takes minutes, they fight
//! over focus and crash each other on a D3D device reset, and neither can be asserted on. For
//! testing whether two players can see each other, none of that is the point.
//!
//! This connects over RakNet, drives the same handshake, and records what arrives. Two probes
//! against one server is a multiplayer test that runs in a second and fails with a message
//! rather than a black window.
//!
//! It is not a client. It renders nothing, holds no world, and only understands the packets
//! needed to answer "what did the server tell me about who else is here".

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use raknet::{message_id, Peer};
use skysaga_proto::bitstream::{BitReader, BitWriter, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{EntityAdd, EntityRemoved, EntitySync, SetClientEntity};

/// The password the server expects, from `SkySaga.Game/Program.cs`. The trailing NUL is part
/// of it.
pub const PASSWORD: &[u8] = b"Something about penguins\0";

/// Client to server ids, already offset by [`ID_USER_PACKET_ENUM`].
pub mod outgoing {
    pub const CLIENT_CONNECTED: u8 = 135;
    pub const CLIENT_READY_TO_SYNC: u8 = 136;
    pub const CLIENT_READY_TO_PLAY: u8 = 137;
    pub const CLIENT_INITIAL_SYNC_FINISHED: u8 = 138;
}

/// Ordinals the probe recognises. Everything else is counted and ignored.
mod incoming {
    /// `SetClientEntity`: which entity the server says is me.
    pub const SET_CLIENT_ENTITY: u16 = 104;
    /// `PlayerJoined`: someone else arrived.
    pub const PLAYER_JOINED: u16 = 25;
    /// `PlayerLeft`: someone else went.
    pub const PLAYER_LEFT: u16 = 26;
    /// `EntityMoved`: something moved.
    pub const ENTITY_MOVED: u16 = 102;
    /// `EntityRemoved`: something is gone. This is what a player leaving looks like; the C#
    /// sends it on disconnect and never sends `PlayerLeft`.
    pub const ENTITY_REMOVED: u16 = 103;
}

/// What a probe was told.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Observations {
    /// The entity the server said is this client, from `SetClientEntity`.
    pub my_entity: Option<u32>,

    /// Every entity id announced by `EntityAdd`, in arrival order.
    pub entities: Vec<u32>,

    /// How many `PlayerJoined` packets arrived.
    pub players_joined: usize,
    /// How many `PlayerLeft` packets arrived.
    pub players_left: usize,
    /// How many `EntityMoved` packets arrived.
    pub entities_moved: usize,
    /// Entity ids the server said are gone.
    pub entities_removed: Vec<u32>,

    /// Every `EntitySync`, by the entity it was about, in arrival order.
    ///
    /// This is what makes the probe usable for anything past the handshake. The client applies
    /// no inventory change locally: it sends a request and waits to be told what happened. So
    /// "did the server act on that packet" is exactly "did a sync for my entity come back",
    /// and that question is the same one against either server.
    pub syncs: Vec<u32>,

    /// Ordinals received but not understood, once each. Useful for noticing that the server
    /// started sending something new.
    pub unhandled: BTreeSet<u16>,
}

impl Observations {
    /// Entities announced to this client that are not its own body.
    ///
    /// This is the question the whole probe exists to answer: with one player it is the
    /// world's props, and with two it should also contain the other player.
    pub fn other_entities(&self) -> Vec<u32> {
        self.entities
            .iter()
            .copied()
            .filter(|id| Some(*id) != self.my_entity)
            .collect()
    }

    /// Whether `entity` was announced to this client.
    pub fn saw_entity(&self, entity: u32) -> bool {
        self.entities.contains(&entity)
    }

    /// Whether this client was told `entity` is gone.
    pub fn saw_removed(&self, entity: u32) -> bool {
        self.entities_removed.contains(&entity)
    }

    /// How many `EntitySync`s arrived for `entity`.
    pub fn syncs_of(&self, entity: u32) -> usize {
        self.syncs.iter().filter(|about| **about == entity).count()
    }

    /// Whether this client's own body was re-synced.
    pub fn my_entity_was_synced(&self) -> bool {
        self.my_entity
            .is_some_and(|entity| self.syncs_of(entity) > 0)
    }
}

/// A headless connection to a game server.
pub struct Probe {
    peer: Peer,
    stage: u8,
    received: usize,
    pub observations: Observations,
}

impl Probe {
    /// Connect, without waiting for the connection to be accepted.
    pub fn connect(host: &str, port: u16) -> anyhow::Result<Self> {
        let peer = Peer::new();

        // Port 0: let the OS choose, so several probes can run at once.
        peer.startup(0, 1)
            .map_err(|error| anyhow::anyhow!("probe peer failed to start: {error:?}"))?;

        peer.connect(host, port, PASSWORD)
            .map_err(|error| anyhow::anyhow!("connect to {host}:{port} failed: {error:?}"))?;

        Ok(Self {
            peer,
            stage: 0,
            received: 0,
            observations: Observations::default(),
        })
    }

    /// Service the connection until `until` says it is done, or the deadline passes.
    ///
    /// Returns whether `until` was satisfied. Polls rather than sleeping a fixed time, so a
    /// test finishes as soon as the thing it waits for happens.
    pub fn run_until(
        &mut self,
        timeout: Duration,
        mut until: impl FnMut(&Observations) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            self.pump();

            if until(&self.observations) {
                return true;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        // One last look: something may have arrived in the final sleep.
        self.pump();

        until(&self.observations)
    }

    /// Run for a fixed time, with no condition. For "and then nothing else happened".
    pub fn run_for(&mut self, timeout: Duration) {
        self.run_until(timeout, |_| false);
    }

    /// Drain whatever has arrived and answer it.
    fn pump(&mut self) {
        loop {
            // A received Packet borrows the peer, and the peer is part of `self`, so nothing
            // taking `&mut self` can be called while one is alive. Copying the bytes out in
            // an inner scope ends that borrow before anything is handled.
            let Some((id, data)) = self
                .peer
                .receive()
                .map(|packet| (packet.message_id(), packet.data().to_vec()))
            else {
                return;
            };

            match id {
                message_id::CONNECTION_REQUEST_ACCEPTED => {
                    self.send(&[outgoing::CLIENT_CONNECTED]);
                    self.stage = 1;
                }

                message_id::CONNECTION_LOST | message_id::DISCONNECTION_NOTIFICATION => {
                    self.stage = 255;
                }

                _ if id >= message_id::ID_USER_PACKET_ENUM => {
                    self.received += 1;
                    self.observe(&data);
                    self.advance();
                }

                _ => {}
            }
        }
    }

    /// Walk the handshake forward. The server answers each step with a burst, so the counts
    /// are how the real client's own pacing is approximated without decoding every packet.
    fn advance(&mut self) {
        match self.stage {
            1 => {
                self.send(&[outgoing::CLIENT_READY_TO_SYNC]);
                self.stage = 2;
            }

            2 if self.received > 4 => {
                self.send(&[outgoing::CLIENT_INITIAL_SYNC_FINISHED]);
                self.stage = 3;
            }

            3 if self.received > 8 => {
                self.send(&[outgoing::CLIENT_READY_TO_PLAY]);
                self.stage = 4;
            }

            _ => {}
        }
    }

    fn observe(&mut self, data: &[u8]) {
        let mut reader = BitReader::from_bytes(data);

        let Ok(ordinal) = reader.read_packet_id() else {
            return;
        };

        match ordinal {
            EntityAdd::ID => {
                if let Ok(add) = EntityAdd::decode(&mut reader) {
                    self.observations.entities.push(add.id);
                }
            }

            incoming::SET_CLIENT_ENTITY => {
                if let Ok(set) = SetClientEntity::decode(&mut reader) {
                    self.observations.my_entity = Some(set.entity_id);
                }
            }

            incoming::PLAYER_JOINED => self.observations.players_joined += 1,
            incoming::PLAYER_LEFT => self.observations.players_left += 1,
            incoming::ENTITY_MOVED => self.observations.entities_moved += 1,

            incoming::ENTITY_REMOVED => {
                if let Ok(removed) = EntityRemoved::decode(&mut reader) {
                    self.observations.entities_removed.push(removed.entity_id);
                }
            }

            EntitySync::ID => {
                // The id alone. Reading the payload would need the entity's definition to
                // know how many flag bits precede it, and which parameter changed is not the
                // question these tests ask -- "did anything come back at all" is.
                if let Ok(sync) = EntitySync::decode(&mut reader) {
                    self.observations.syncs.push(sync.id);
                }
            }

            other => {
                self.observations.unhandled.insert(other + ID_USER_PACKET_ENUM);
            }
        }
    }

    /// Send to the server.
    ///
    /// `broadcast` rather than `send`: a client peer has exactly one connection, and
    /// addressing it by guid would mean tracking a guid to no purpose.
    pub fn send(&self, data: &[u8]) {
        self.peer.broadcast(data);
    }

    /// Encode a packet and send it.
    ///
    /// The encoders in `skysaga-proto` are shared with the server, which decodes the same
    /// bytes -- so a layout that is wrong here is wrong there too, and the round trip is
    /// covered by that crate's own golden tests rather than by trusting this.
    pub fn send_packet(&self, write: impl FnOnce(&mut BitWriter)) {
        let mut writer = BitWriter::new();

        write(&mut writer);

        self.send(&writer.into_bytes());
    }

    /// The entity the server said is this client, once it has said so.
    pub fn my_entity(&self) -> Option<u32> {
        self.observations.my_entity
    }

    /// Run until the server has handed over a player entity, i.e. the handshake finished.
    ///
    /// Returns whether it did. Everything past the handshake needs this first: a packet about
    /// "my entity" cannot be sent before the server has said which entity that is.
    pub fn wait_for_world(&mut self, timeout: Duration) -> bool {
        self.run_until(timeout, |seen| seen.my_entity.is_some())
    }

    /// Forget what has been seen so far.
    ///
    /// Called after the handshake so an assertion about "what that packet caused" is not
    /// satisfied by something from the burst that preceded it.
    pub fn forget(&mut self) {
        let my_entity = self.observations.my_entity;

        self.observations = Observations {
            my_entity,
            ..Default::default()
        };
    }

    /// Close the connection, so the server sees the client go rather than time it out.
    pub fn disconnect(&self) {
        self.peer.shutdown(100);
    }
}
