//! A safe wrapper over the small piece of RakNet the emulator uses.
//!
//! This crate is the *only* place `unsafe` appears in the port. Everything above it works in
//! `&[u8]`, and serialisation is `skysaga-proto`'s pure-Rust `BitStream` — RakNet's own
//! `BitStream` is never touched, which is what keeps this surface to a dozen calls.
//!
//! ```no_run
//! # use raknet::Peer;
//! let peer = Peer::new();
//! peer.set_maximum_incoming_connections(64);
//! peer.startup(42069, 64).expect("bind");
//!
//! while let Some(packet) = peer.receive() {
//!     println!("{} bytes from {:?}", packet.data().len(), packet.guid());
//! }
//! ```
//!
//! # Ownership
//!
//! RakNet hands out pointers the caller must give back. Both are `Drop` here, so neither can
//! be leaked or double-freed: [`Peer`] destroys its instance, and [`Packet`] deallocates
//! itself. The C# equivalent uses `goto Deallocate` and leaks on any early return.
//!
//! # Threading
//!
//! RakNet runs its own threads and is internally synchronised, so `Peer` is `Send + Sync` and
//! its methods take `&self`. `Packet` borrows its `Peer` and is deliberately **not** `Send`:
//! it points into RakNet-owned memory that must be released on the receiving side.

use std::ffi::{c_char, CString};
use std::marker::PhantomData;

use thiserror::Error;

/// RakNet's built-in message ids, for the ones a server actually branches on.
///
/// Game packets start at [`ID_USER_PACKET_ENUM`]; see `skysaga_proto::bitstream`.
pub mod message_id {
    /// The client's connection request succeeded (seen by the connecting side).
    pub const CONNECTION_REQUEST_ACCEPTED: u8 = 16;
    /// A peer connected to us (seen by the listening side).
    pub const NEW_INCOMING_CONNECTION: u8 = 19;
    /// A peer went away cleanly.
    pub const DISCONNECTION_NOTIFICATION: u8 = 21;
    /// A peer stopped responding.
    pub const CONNECTION_LOST: u8 = 22;
    /// First id available to the game.
    pub const ID_USER_PACKET_ENUM: u8 = 134;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StartupError {
    /// `Startup` returned a non-zero `StartupResult`. The common one is `5`,
    /// `SOCKET_PORT_ALREADY_IN_USE`.
    #[error("RakNet startup failed with StartupResult {0}")]
    Failed(i32),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConnectError {
    /// `Connect` returned a non-zero `ConnectionAttemptResult`.
    #[error("RakNet connect failed with ConnectionAttemptResult {0}")]
    Failed(i32),

    #[error("host contained an interior NUL")]
    BadHost,
}

/// A peer's 64-bit identity. Stable for the lifetime of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub u64);

/// A RakNet peer: a bound UDP socket plus its connection table.
#[derive(Debug)]
pub struct Peer {
    raw: *mut std::ffi::c_void,
}

// RakNet is internally synchronised; the C# emulator relies on this too.
unsafe impl Send for Peer {}
unsafe impl Sync for Peer {}

impl Peer {
    pub fn new() -> Self {
        Self {
            // SAFETY: GetInstance allocates and returns a valid peer, or aborts internally.
            raw: unsafe { raknet_sys::CSharp_RakPeerInterface_GetInstance() },
        }
    }

    /// Bind a UDP socket. `port` of 0 lets the OS choose — see [`Self::port`].
    pub fn startup(&self, port: u16, max_connections: u16) -> Result<(), StartupError> {
        // SAFETY: the descriptor is valid for the duration of the Startup call and deleted
        // immediately after; RakNet copies what it needs.
        let result = unsafe {
            let descriptor = raknet_sys::CSharp_new_SocketDescriptor__SWIG_1(port, c"".as_ptr());

            let result = raknet_sys::CSharp_RakPeerInterface_Startup__SWIG_1(
                self.raw,
                u32::from(max_connections),
                descriptor,
                1,
            );

            raknet_sys::CSharp_delete_SocketDescriptor(descriptor);

            result
        };

        if result == raknet_sys::RAKNET_STARTED {
            Ok(())
        } else {
            Err(StartupError::Failed(result))
        }
    }

    /// The port actually bound, which is what you need after starting on port 0.
    pub fn port(&self) -> Option<u16> {
        // SAFETY: GetInternalID returns an owned SystemAddress that we delete.
        let port = unsafe {
            let address = raknet_sys::CSharp_RakPeerInterface_GetInternalID__SWIG_2(self.raw);

            if address.is_null() {
                return None;
            }

            let port = raknet_sys::CSharp_SystemAddress_GetPort(address);

            raknet_sys::CSharp_delete_SystemAddress(address);

            port
        };

        (port != 0).then_some(port)
    }

    /// How many incoming connections to accept. Must be set before [`Self::startup`] for a
    /// listening peer, and left at zero for a pure client.
    pub fn set_maximum_incoming_connections(&self, count: u16) {
        // SAFETY: self.raw is a live peer.
        unsafe { raknet_sys::CSharp_RakPeerInterface_SetMaximumIncomingConnections(self.raw, count) }
    }

    /// The password incoming connections must present. An interior NUL truncates it, which is
    /// what RakNet would do with a C string anyway.
    pub fn set_incoming_password(&self, password: &str) {
        let bytes = CString::new(password).unwrap_or_default();

        // SAFETY: the string outlives the call; RakNet copies it.
        unsafe {
            raknet_sys::CSharp_RakPeerInterface_SetIncomingPassword__SWIG_0(
                self.raw,
                bytes.as_ptr(),
                password.len() as i32,
            )
        }
    }

    /// Begin connecting to a listening peer. Returns as soon as the attempt is *started*;
    /// success arrives later as [`message_id::CONNECTION_REQUEST_ACCEPTED`].
    pub fn connect(&self, host: &str, port: u16, password: &str) -> Result<(), ConnectError> {
        let host = CString::new(host).map_err(|_| ConnectError::BadHost)?;
        let password_bytes = CString::new(password).unwrap_or_default();

        // SAFETY: both strings outlive the call.
        let result = unsafe {
            raknet_sys::CSharp_RakPeerInterface_Connect__SWIG_0(
                self.raw,
                host.as_ptr(),
                port,
                password_bytes.as_ptr(),
                password.len() as i32,
                std::ptr::null_mut(),
                0,
                12,   // attempts
                500,  // ms between attempts
                0,    // no timeout override
            )
        };

        if result == 0 {
            Ok(())
        } else {
            Err(ConnectError::Failed(result))
        }
    }

    /// The next packet, or `None` when the queue is empty. Never blocks.
    ///
    /// Drain until this returns `None` — the C# emulator takes one packet per 30 ms tick,
    /// which caps the whole server at about 33 packets a second.
    pub fn receive(&self) -> Option<Packet<'_>> {
        // SAFETY: Receive returns either null or a packet we now own.
        let raw = unsafe { raknet_sys::CSharp_RakPeerInterface_Receive(self.raw) };

        (!raw.is_null()).then(|| Packet { peer: self, raw, _not_send: PhantomData })
    }

    /// Send to one peer. Returns 0 if RakNet refused it.
    pub fn send(&self, guid: Guid, data: &[u8]) -> u32 {
        self.send_inner(Some(guid), data, false)
    }

    /// Send to every connected peer.
    pub fn broadcast(&self, data: &[u8]) -> u32 {
        self.send_inner(None, data, true)
    }

    fn send_inner(&self, guid: Option<Guid>, data: &[u8], broadcast: bool) -> u32 {
        if data.is_empty() {
            return 0;
        }

        // SAFETY: the address is built and destroyed around the call; `data` is only read
        // for `data.len()` bytes, and RakNet copies it before returning.
        //
        // The system identifier must be a real object even when broadcasting -- RakNet
        // dereferences it either way, so a null pointer here is a segfault rather than an
        // "ignored" argument.
        unsafe {
            let (guid_raw, address) = match guid {
                Some(Guid(value)) => {
                    let guid_raw = raknet_sys::CSharp_new_RakNetGUID__SWIG_1(value);
                    let address = raknet_sys::CSharp_new_AddressOrGUID__SWIG_4(guid_raw);

                    (guid_raw, address)
                }
                None => (
                    std::ptr::null_mut(),
                    raknet_sys::CSharp_new_AddressOrGUID__SWIG_0(),
                ),
            };

            let sent = raknet_sys::CSharp_RakPeerInterface_Send__SWIG_0(
                self.raw,
                data.as_ptr() as *const c_char,
                data.len() as i32,
                raknet_sys::HIGH_PRIORITY,
                raknet_sys::RELIABLE_ORDERED,
                0, // ordering channel -- SkySaga only ever uses 0
                address,
                u32::from(broadcast),
                0,
            );

            raknet_sys::CSharp_delete_AddressOrGUID(address);

            if !guid_raw.is_null() {
                raknet_sys::CSharp_delete_RakNetGUID(guid_raw);
            }

            sent
        }
    }

    pub fn connection_count(&self) -> u16 {
        // SAFETY: self.raw is a live peer.
        unsafe { raknet_sys::CSharp_RakPeerInterface_NumberOfConnections(self.raw) }
    }

    /// This peer's own guid.
    pub fn guid(&self) -> Guid {
        // SAFETY: GetMyGUID returns an owned RakNetGUID that we delete.
        unsafe {
            let raw = raknet_sys::CSharp_RakPeerInterface_GetMyGUID(self.raw);
            let value = raknet_sys::CSharp_RakNetGUID_g_get(raw);

            raknet_sys::CSharp_delete_RakNetGUID(raw);

            Guid(value)
        }
    }

    /// Stop accepting traffic, letting queued sends drain for `block_ms`.
    pub fn shutdown(&self, block_ms: u32) {
        // SAFETY: self.raw is a live peer.
        unsafe { raknet_sys::CSharp_RakPeerInterface_Shutdown__SWIG_2(self.raw, block_ms) }
    }
}

impl Default for Peer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        // SAFETY: self.raw came from GetInstance and is destroyed exactly once. Shutdown
        // first so RakNet's threads stop before the instance goes away.
        unsafe {
            raknet_sys::CSharp_RakPeerInterface_Shutdown__SWIG_2(self.raw, 100);
            raknet_sys::CSharp_RakPeerInterface_DestroyInstance(self.raw);
        }
    }
}

/// A received packet. Deallocated when dropped.
#[derive(Debug)]
pub struct Packet<'a> {
    peer: &'a Peer,
    raw: *mut std::ffi::c_void,
    /// Points into RakNet-owned memory that must be released on the receiving side.
    _not_send: PhantomData<*const ()>,
}

impl Packet<'_> {
    /// The payload, message id included — `data()[0]` is what RakNet dispatches on.
    pub fn data(&self) -> &[u8] {
        // SAFETY: the packet is alive for as long as `self`, and RakNet guarantees `data` is
        // valid for `length` bytes.
        unsafe {
            let pointer = raknet_sys::CSharp_Packet_data_get(self.raw);
            let length = raknet_sys::CSharp_Packet_length_get(self.raw) as usize;

            if pointer.is_null() || length == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(pointer, length)
            }
        }
    }

    /// The first byte, or 0 for an empty packet. See [`message_id`].
    pub fn message_id(&self) -> u8 {
        self.data().first().copied().unwrap_or(0)
    }

    /// Who sent it.
    pub fn guid(&self) -> Guid {
        // SAFETY: the guid is borrowed from the packet, so it is not deleted here.
        unsafe {
            let raw = raknet_sys::CSharp_Packet_guid_get(self.raw);

            if raw.is_null() {
                Guid(0)
            } else {
                Guid(raknet_sys::CSharp_RakNetGUID_g_get(raw))
            }
        }
    }
}

impl Drop for Packet<'_> {
    fn drop(&mut self) {
        // SAFETY: each packet is deallocated exactly once, against the peer that produced it.
        unsafe {
            raknet_sys::CSharp_RakPeerInterface_DeallocatePacket(self.peer.raw, self.raw);
        }
    }
}
