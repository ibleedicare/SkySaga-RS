//! Raw FFI declarations for the SLikeNet entrypoints the emulator needs.
//!
//! # Why there is no C++ shim
//!
//! `libRakNet.so` is SLikeNet built together with its `raknet_backwards_compatibility` SWIG
//! wrapper, which exports **1879 unmangled `extern "C"` functions**. Rust can call those
//! directly, so no shim has to be written or built.
//!
//! The original plan was a hand-written C++ shim, on the grounds that the SWIG wrapper
//! carries the LP64 `long` bug — `Write< long >` is 32 bits on MSVC and 64 on Linux, which
//! made every packet containing an `int` four bytes too long. Two things retire that concern:
//!
//! 1. The flake already narrows `long` to `int32_t` when building the wrapper.
//! 2. **Nothing here touches RakNet's `BitStream`.** Serialisation is `skysaga-proto`'s
//!    pure-Rust implementation, so the only surface used is byte-oriented send and receive —
//!    which the bug never affected. The roadmap's ~30-function shim collapses to the 18
//!    declarations below.
//!
//! # Conventions
//!
//! SWIG's C# generator maps `bool` to a 4-byte value, so booleans are `u32` here. Objects are
//! opaque `*mut c_void`; every `new_*` has a matching `delete_*` and the safe wrapper in
//! `raknet` pairs them with `Drop`.
//!
//! Enum ordinals used by callers (from the generated C# enums):
//!
//! | value | meaning |
//! |---|---|
//! | `PacketPriority::HIGH_PRIORITY` | 1 |
//! | `PacketReliability::RELIABLE_ORDERED` | 3 |
//! | `StartupResult::RAKNET_STARTED` | 0 |

#![allow(non_snake_case)]

use std::ffi::{c_char, c_int, c_void};

/// `PacketPriority::HIGH_PRIORITY`. The only priority SkySaga sends.
pub const HIGH_PRIORITY: c_int = 1;

/// `PacketReliability::RELIABLE_ORDERED`. The only reliability SkySaga sends.
pub const RELIABLE_ORDERED: c_int = 3;

/// `StartupResult::RAKNET_STARTED`.
pub const RAKNET_STARTED: c_int = 0;

extern "C" {
    // --- peer lifecycle ---------------------------------------------------------------

    pub fn CSharp_RakPeerInterface_GetInstance() -> *mut c_void;
    pub fn CSharp_RakPeerInterface_DestroyInstance(peer: *mut c_void);

    /// `Startup(maxConnections, socketDescriptors, socketDescriptorCount)`.
    /// Returns a `StartupResult`; `0` is success.
    pub fn CSharp_RakPeerInterface_Startup__SWIG_1(
        peer: *mut c_void,
        max_connections: u32,
        socket_descriptors: *mut c_void,
        socket_descriptor_count: u32,
    ) -> c_int;

    /// `Shutdown(blockDuration)`.
    pub fn CSharp_RakPeerInterface_Shutdown__SWIG_2(peer: *mut c_void, block_duration: u32);

    pub fn CSharp_RakPeerInterface_SetMaximumIncomingConnections(peer: *mut c_void, count: u16);

    pub fn CSharp_RakPeerInterface_SetIncomingPassword__SWIG_0(
        peer: *mut c_void,
        password: *const c_char,
        password_length: c_int,
    );

    pub fn CSharp_RakPeerInterface_NumberOfConnections(peer: *mut c_void) -> u16;

    /// This peer's own guid, as an owned `RakNetGUID` that must be deleted.
    pub fn CSharp_RakPeerInterface_GetMyGUID(peer: *mut c_void) -> *mut c_void;

    /// The port actually bound. Needed when starting on port 0.
    pub fn CSharp_RakPeerInterface_GetInternalID__SWIG_2(peer: *mut c_void) -> *mut c_void;

    // --- connecting (the test harness and any client-side tooling) ----------------------

    /// `Connect(host, remotePort, passwordData, passwordDataLength, publicKey,
    /// connectionSocketIndex, sendConnectionAttemptCount, timeBetweenSendConnectionAttemptsMS,
    /// timeoutTime)`. Returns a `ConnectionAttemptResult`; `0` is `CONNECTION_ATTEMPT_STARTED`.
    pub fn CSharp_RakPeerInterface_Connect__SWIG_0(
        peer: *mut c_void,
        host: *const c_char,
        remote_port: u16,
        password: *const c_char,
        password_length: c_int,
        public_key: *mut c_void,
        connection_socket_index: u32,
        send_connection_attempt_count: u32,
        time_between_attempts_ms: u32,
        timeout_time: u32,
    ) -> c_int;

    // --- traffic ------------------------------------------------------------------------

    /// The next packet, or null. The caller owns it until `DeallocatePacket`.
    pub fn CSharp_RakPeerInterface_Receive(peer: *mut c_void) -> *mut c_void;

    pub fn CSharp_RakPeerInterface_DeallocatePacket(peer: *mut c_void, packet: *mut c_void);

    /// `Send(data, length, priority, reliability, orderingChannel, systemIdentifier,
    /// broadcast, forceReceiptNumber)`. Returns 0 when the send was refused.
    ///
    /// This is the byte-oriented overload — the other one takes a native `BitStream`, which
    /// is deliberately not used.
    pub fn CSharp_RakPeerInterface_Send__SWIG_0(
        peer: *mut c_void,
        data: *const c_char,
        length: c_int,
        priority: c_int,
        reliability: c_int,
        ordering_channel: c_char,
        system_identifier: *mut c_void,
        broadcast: u32,
        force_receipt_number: u32,
    ) -> u32;

    // --- packet accessors -----------------------------------------------------------------

    pub fn CSharp_Packet_data_get(packet: *mut c_void) -> *mut u8;
    pub fn CSharp_Packet_length_get(packet: *mut c_void) -> u32;

    /// Borrowed — points into the packet, so it must not be deleted.
    pub fn CSharp_Packet_guid_get(packet: *mut c_void) -> *mut c_void;

    // --- value types ----------------------------------------------------------------------

    /// `SocketDescriptor(port, hostAddress)`.
    pub fn CSharp_new_SocketDescriptor__SWIG_1(port: u16, host: *const c_char) -> *mut c_void;
    pub fn CSharp_delete_SocketDescriptor(descriptor: *mut c_void);

    pub fn CSharp_new_RakNetGUID__SWIG_1(guid: u64) -> *mut c_void;
    pub fn CSharp_delete_RakNetGUID(guid: *mut c_void);
    pub fn CSharp_RakNetGUID_g_get(guid: *mut c_void) -> u64;

    /// Default-constructed, i.e. UNASSIGNED. Required for a broadcast: RakNet dereferences
    /// the system identifier even when broadcasting, so it must not be null.
    pub fn CSharp_new_AddressOrGUID__SWIG_0() -> *mut c_void;

    /// `AddressOrGUID(RakNetGUID)` — how a send is addressed to one peer.
    ///
    /// Note the overload number: `__SWIG_2` takes a `SystemAddress`, not a guid. SWIG
    /// numbers overloads in declaration order, so they are not guessable — check the
    /// generated C# in `SkySaga.RakNet/` before adding a binding here.
    pub fn CSharp_new_AddressOrGUID__SWIG_4(guid: *mut c_void) -> *mut c_void;
    pub fn CSharp_delete_AddressOrGUID(address: *mut c_void);

    pub fn CSharp_SystemAddress_GetPort(address: *mut c_void) -> u16;
    pub fn CSharp_delete_SystemAddress(address: *mut c_void);
}
