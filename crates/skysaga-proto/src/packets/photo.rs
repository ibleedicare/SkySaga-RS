//! Photos: capture validation.
//!
//! # Why character creation depends on this
//!
//! The last thing the client does in its creator is capture the character's portrait. It
//! sends [`NotifyPhotoCaptured`] and then *waits*: the capture stays in a pending queue, and
//! the client will not upload it — or leave `GameState_CharacterCreation` — until a
//! [`PhotoValidated`] echoes the id back with somewhere to upload to.
//!
//! With this unimplemented the client sits on the "Character Creation" loading screen
//! indefinitely, its own log reading `Photo capture started: 1 waiting in total` with no
//! matching state exit. That looks exactly like a broken character creator, and it is not:
//! creation itself has already succeeded by that point.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// Bits of position and direction between the id and the avatar flag, *if* they are six raw
/// 32-bit floats.
///
/// They may not be: RakNet can write vectors compressed, and a live client's capture was
/// shorter than this, which made a fixed skip overrun and fail the whole decode. The flag is
/// therefore read only when the packet is long enough, and the layout is not asserted.
const TRANSFORM_BITS: u32 = 6 * 32;

/// The client captured a photo and wants somewhere to put it.
///
/// Layout from the client's serialiser (`FUN_00791e60`):
///
/// ```text
/// clientPhotoID   compressed uint
/// position        3 floats
/// direction       3 floats
/// isAvatarPhoto   1 bit
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyPhotoCaptured {
    /// The client's own id for the capture. Echoed back verbatim so it can match the reply
    /// to the pending photo.
    pub client_photo_id: u32,

    /// Set for the character portrait taken during creation, clear for an in-game snapshot.
    pub is_avatar_photo: bool,
}

impl NotifyPhotoCaptured {
    pub const ID: u16 = 150;

    /// Decode a capture.
    ///
    /// **Only `clientPhotoID` is required.** Everything after it is positional detail the
    /// server does not need to validate a photo, and the C# reads none of it either. Failing
    /// the decode over a trailing field the server ignores would drop the reply — and a
    /// dropped reply strands the client in character creation forever, which is precisely the
    /// bug this packet exists to fix. So the avatar flag is best-effort: read when the packet
    /// is long enough for the layout to be as documented, `false` otherwise.
    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        let client_photo_id = reader.read_compressed_u32()?;

        let is_avatar_photo = reader
            .skip_bits(TRANSFORM_BITS)
            .and_then(|_| reader.read_bit())
            .unwrap_or(false);

        Ok(Self {
            client_photo_id,
            is_avatar_photo,
        })
    }
}

/// The photo was accepted: here is its id and the token to upload it with.
///
/// Layout from the client's deserialiser (`FUN_0073f880`, which logs `RPCPhotoValidated`):
///
/// ```text
/// clientPhotoID   compressed uint   echoed from the capture
/// officialUUID    string            PUT the image to
///                                   /api/binary-storage/photos/<officialUUID>/_upload
/// uploadToken     string            sent along with the upload
/// ```
///
/// Unlike most packets here, `encode` writes its own id: it is never embedded in another
/// stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoValidated {
    pub client_photo_id: u32,
    pub official_uuid: String,
    pub upload_token: String,
}

impl PhotoValidated {
    pub const ID: u16 = 152;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_compressed_u32(self.client_photo_id);

        writer.write_string(&self.official_uuid);
        writer.write_string(&self.upload_token);
    }
}
