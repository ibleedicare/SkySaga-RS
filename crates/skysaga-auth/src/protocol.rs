//! The Smilegate login wire format.
//!
//! Reversed from the C#'s `[StructLayout(LayoutKind.Sequential, Pack = 1)]` marshalling in
//! `SmilegateAuth/Packet/`. Everything is little-endian and byte-packed; strings are
//! fixed-width NUL-padded ASCII (`ByValTStr`).
//!
//! ```text
//! Header                                       5 bytes
//!   0   u8      magic   = 0xF1
//!   1   u16     length            total packet length, header included
//!   3   u16     id                0x0312 LoginRequest, 0x0412 LoginReply
//!
//! LoginRequest                               123 bytes
//!   5   i32     unknown
//!   9   [u8;32] unknown2
//!  41   [u8;50] username
//!  91   [u8;32] password
//!
//! LoginReply                                1095 bytes
//!   5   i32     result            see LoginResult
//!   9   i32     unknown
//!  13   [u8;8]  gap               zeroes
//!  21   [u8;50] username
//!  71   [u8;1024] token
//! ```

use skysaga_core::{fixed_str, Reader, ReaderError};
use thiserror::Error;

/// First byte of every packet.
pub const MAGIC: u8 = 0xF1;

pub const HEADER_SIZE: usize = 5;

pub mod packet_id {
    pub const LOGIN_REQUEST: u16 = 0x0312;
    pub const LOGIN_REPLY: u16 = 0x0412;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("bad magic byte {0:#04x}, expected {MAGIC:#04x}")]
    BadMagic(u8),

    #[error("header declares {declared} bytes but {actual} were received")]
    LengthMismatch { declared: usize, actual: usize },

    #[error("unexpected packet id {0:#06x}")]
    UnexpectedId(u16),

    #[error(transparent)]
    Truncated(#[from] ReaderError),
}

/// The five-byte header every packet starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub length: u16,
    pub id: u16,
}

impl Header {
    /// Parse a header from exactly [`HEADER_SIZE`] bytes, validating the magic byte.
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(bytes);

        let magic = reader.u8()?;

        if magic != MAGIC {
            return Err(ProtocolError::BadMagic(magic));
        }

        Ok(Self {
            length: reader.u16_le()?,
            id: reader.u16_le()?,
        })
    }

    /// Number of bytes that follow the header.
    ///
    /// Saturates rather than underflowing on a header that claims to be shorter than a
    /// header, which a hostile peer can send.
    pub fn body_len(self) -> usize {
        usize::from(self.length).saturating_sub(HEADER_SIZE)
    }
}

/// `LoginRequest` — the only packet the client sends here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoginRequest {
    pub unknown: i32,
    pub unknown2: String,
    pub username: String,
    pub password: String,
}

impl LoginRequest {
    pub const SIZE: usize = 123;

    /// Parse a whole packet, header included.
    pub fn parse(packet: &[u8]) -> Result<Self, ProtocolError> {
        let header = Header::parse(packet.get(..HEADER_SIZE).unwrap_or(packet))?;

        if usize::from(header.length) != packet.len() {
            return Err(ProtocolError::LengthMismatch {
                declared: usize::from(header.length),
                actual: packet.len(),
            });
        }

        if header.id != packet_id::LOGIN_REQUEST {
            return Err(ProtocolError::UnexpectedId(header.id));
        }

        Self::parse_body(&packet[HEADER_SIZE..])
    }

    /// Parse the 118 bytes that follow the header.
    pub fn parse_body(body: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(body);

        Ok(Self {
            unknown: reader.i32_le()?,
            unknown2: reader.fixed_str::<32>()?,
            username: reader.fixed_str::<50>()?,
            password: reader.fixed_str::<32>()?,
        })
    }

    /// Serialize, header included. Only used by the tests and by client-side tooling.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];

        out[0] = MAGIC;
        out[1..3].copy_from_slice(&(Self::SIZE as u16).to_le_bytes());
        out[3..5].copy_from_slice(&packet_id::LOGIN_REQUEST.to_le_bytes());
        out[5..9].copy_from_slice(&self.unknown.to_le_bytes());

        fixed_str::write(&mut out[9..41], &self.unknown2);
        fixed_str::write(&mut out[41..91], &self.username);
        fixed_str::write(&mut out[91..123], &self.password);

        out
    }
}

/// Values of [`LoginReply::result`].
///
/// The text is what the client renders, transcribed from the C# comment block; the bracketed
/// tags are the client's own message keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum LoginResult {
    /// DB query error.
    DatabaseError = -2,
    /// ID does not exist.
    NoSuchAccount = -1,
    Ok = 0,
    /// The password is incorrect.
    WrongPassword = 1,
    /// The following are not allowed IP.
    IpBlocked = 2,
    /// `[USER_BLOCKED]`
    UserBlocked = 3,
    /// `[COUNTRY_BLOCKED]` — SkySaga is not available in your region.
    CountryBlocked = 4,
    /// `[PWD_FAIL_BLOCKED]` — password locked.
    PasswordLocked = 5,
    /// `[INVALID_SPOT_CODE]`
    InvalidSpotCode = 6,
    /// `[WITHDRAW_MEMBER]` — cancelled account.
    AccountCancelled = 7,
    /// `[under maintenance]`
    Maintenance = 8,
    /// `[NOT_CBT_MEMBER]` — not approved for the Closed Beta Test.
    NotBetaMember = 9,
    /// `[BAN_USER]`
    Banned = 10,
}

/// `LoginReply` — the only packet the server sends here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginReply {
    pub result: LoginResult,
    pub unknown: i32,
    pub username: String,
    pub token: String,
}

impl LoginReply {
    pub const SIZE: usize = 1095;

    pub fn accepted(username: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            result: LoginResult::Ok,
            unknown: 0,
            username: username.into(),
            token: token.into(),
        }
    }

    pub fn rejected(username: impl Into<String>, result: LoginResult) -> Self {
        Self {
            result,
            unknown: 0,
            username: username.into(),
            token: String::new(),
        }
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];

        out[0] = MAGIC;
        out[1..3].copy_from_slice(&(Self::SIZE as u16).to_le_bytes());
        out[3..5].copy_from_slice(&packet_id::LOGIN_REPLY.to_le_bytes());
        out[5..9].copy_from_slice(&(self.result as i32).to_le_bytes());
        out[9..13].copy_from_slice(&self.unknown.to_le_bytes());
        // out[13..21] is the eight-byte gap, already zero.

        fixed_str::write(&mut out[21..71], &self.username);
        fixed_str::write(&mut out[71..1095], &self.token);

        out
    }

    /// Parse a reply. The server never needs this; the tests and the launcher-side tooling do.
    pub fn parse(packet: &[u8]) -> Result<Self, ProtocolError> {
        let header = Header::parse(packet.get(..HEADER_SIZE).unwrap_or(packet))?;

        if header.id != packet_id::LOGIN_REPLY {
            return Err(ProtocolError::UnexpectedId(header.id));
        }

        let mut reader = Reader::new(&packet[HEADER_SIZE..]);

        let result = match reader.i32_le()? {
            -2 => LoginResult::DatabaseError,
            -1 => LoginResult::NoSuchAccount,
            0 => LoginResult::Ok,
            1 => LoginResult::WrongPassword,
            2 => LoginResult::IpBlocked,
            3 => LoginResult::UserBlocked,
            4 => LoginResult::CountryBlocked,
            5 => LoginResult::PasswordLocked,
            6 => LoginResult::InvalidSpotCode,
            7 => LoginResult::AccountCancelled,
            8 => LoginResult::Maintenance,
            9 => LoginResult::NotBetaMember,
            _ => LoginResult::Banned,
        };

        let unknown = reader.i32_le()?;

        reader.bytes(8)?;

        Ok(Self {
            result,
            unknown,
            username: reader.fixed_str::<50>()?,
            token: reader.fixed_str::<1024>()?,
        })
    }
}
