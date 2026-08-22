//! The mailbox.
//!
//! Seven packets carrying two strings between them. **All the mail data travels the other
//! way**, as `Player.mailitemlist` inside an ordinary `EntitySync` -- the whole inbox is one
//! entity parameter, sync index 50. So these are requests and acknowledgements, and there is
//! no mail packet as such.
//!
//! Mail is 100% RakNet: there is no HTTP endpoint for any of it.
//!
//! # The two that look like they do nothing
//!
//! [`MailCheck`] and [`RemoteMailSynced`] are both a bare id, and both are load-bearing.
//!
//! `MailCheck` is what the panel sends on opening; while it went unanswered the panel span on
//! "loading" forever. `RemoteMailSynced` is what stops it doing that -- the client's handler
//! sets a flag, and until it is set the panel renders its loading state and draws no rows
//! **even when `mailitemlist` was synced perfectly**. It has to follow the sync carrying the
//! list; the channel is reliable-ordered, so sending them in order is enough.
//!
//! Reversed in `documentations/mail.md`, verified live on 2026-08-20.

use crate::bitstream::{BitError, BitReader, BitWriter};

/// `NewMailRecieved` -- the doorbell. Server to client.
///
/// Spelling follows the client's own RPC name.
///
/// The uuid it carries is **thrown away**: the client's handler does nothing but send
/// [`MailCheck`] straight back, byte-identical to the panel-opened path. It is still read off
/// the stream, so an absent one desynchronises the reader rather than being ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMailReceived {
    pub message_uuid: String,
}

impl NewMailReceived {
    pub const ID: u16 = 93;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_string(&self.message_uuid);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            message_uuid: reader.read_string()?,
        })
    }
}

/// `MailRead` -- the player opened a message.
///
/// The client sets its own read bit *before* sending, so there is no reply to make. The server
/// must still record it: the next re-sync of `mailitemlist` with the bit clear pops the
/// message back to unread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailRead {
    pub message_uuid: String,
}

/// `MailGiftSelected` -- the player picked one of the gift options.
///
/// **It does not say which gift.** The UI callback gets the clicked index, highlights the
/// button and sends this; the index never reaches the wire. All it means is "a choice was
/// made", which sets flag bit 3 and stops the client offering the buttons again. The choice
/// itself is committed by the [`TakeMailAttachment`] that follows, whose item uuid does
/// identify what was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailGiftSelected {
    pub message_uuid: String,
}

/// `DeleteMail` -- discard a message.
///
/// The client counts unclaimed attachments first and raises a confirmation, so this only
/// arrives for a discard the player agreed to. It does **not** remove the row itself: the row
/// disappears when the server re-syncs the list without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteMail {
    pub message_uuid: String,
}

/// One string, for the three packets whose whole body is a message uuid.
macro_rules! message_uuid_packet {
    ($name:ident, $id:expr) => {
        impl $name {
            pub const ID: u16 = $id;

            pub fn encode(&self, writer: &mut BitWriter) {
                writer.write_packet_id(Self::ID);
                writer.write_string(&self.message_uuid);
            }

            pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
                Ok(Self {
                    message_uuid: reader.read_string()?,
                })
            }
        }
    };
}

message_uuid_packet!(MailRead, 94);
message_uuid_packet!(MailGiftSelected, 95);
message_uuid_packet!(DeleteMail, 97);

/// `MailCheck` -- "send me my inbox". No body at all; the id is the whole message.
///
/// Sent when the mailbox panel opens, and again whenever a [`NewMailReceived`] doorbell
/// arrives. While it went unanswered the panel span on "loading" forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailCheck;

impl MailCheck {
    pub const ID: u16 = 96;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }

    pub fn decode(_reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self)
    }
}

/// `TakeMailAttachment` -- claim one attachment into the rucksack.
///
/// Two strings, message first. The client blanks its own copy of the slot optimistically
/// before sending, so silence here does not leave the icon on screen -- it leaves the item
/// *nowhere* until the next re-bind puts it back.
///
/// It is a **drag and drop, not a click**: the client sends this only from its drop branch, so
/// a click produces no packet at all and a silent server log means the gesture was wrong
/// rather than the handler missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeMailAttachment {
    pub message_uuid: String,
    pub item_uuid: String,
}

impl TakeMailAttachment {
    pub const ID: u16 = 98;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
        writer.write_string(&self.message_uuid);
        writer.write_string(&self.item_uuid);
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            message_uuid: reader.read_string()?,
            item_uuid: reader.read_string()?,
        })
    }
}

/// `RemoteMailSynced` -- "your inbox is up to date". Server to client, no body.
///
/// **The packet that stops the panel saying "loading".** Its handler sets a flag, and until
/// that flag is set the panel draws no rows even when `mailitemlist` was synced perfectly.
/// Must follow the sync carrying the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteMailSynced;

impl RemoteMailSynced {
    pub const ID: u16 = 99;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }
}
