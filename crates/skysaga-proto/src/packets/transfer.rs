//! Moving a client to another world.
//!
//! `TransferToServer` is how the server tells a connected client "your world is somewhere
//! else, go there". Its four fields are the same ones `game-conductor/retrieve` hands out
//! over HTTP — the documentation puts it plainly: *`TransferToServer` is `retrieve`'s
//! response body*. There is no new protocol here.
//!
//! Reversed in `documentations/worlds-teleport-and-pvp.md` §2.1, from the client's
//! deserialiser `FUN_0073eec0` and the handler `FUN_007398c0`, which calls
//! `WorldConnection::Set(serverUUID, worldUUID, ip, port)`.

use crate::bitstream::BitWriter;

/// Server → client: reconnect to this world, on this server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferToServer {
    pub server_uuid: String,
    pub world_uuid: String,
    pub ip: String,
    /// Written as 32 bits and read back as 16 by the client's setter, which stores only the
    /// low word. The full 32 are still written — that is what its reader consumes, and
    /// writing fewer would desynchronise the packet.
    pub port: u16,
}

impl TransferToServer {
    pub const ID: u16 = 12;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_string(&self.server_uuid);
        writer.write_string(&self.world_uuid);
        writer.write_string(&self.ip);

        writer.write_u32(self.port as u32);
    }
}
