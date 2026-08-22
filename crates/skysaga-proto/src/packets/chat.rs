//! The RakNet half of chat.
//!
//! **Chat is two transports and neither works alone.** This one carries the *list of channels*
//! and nothing else; every actual message goes over a separate TCP socket speaking IRC. So
//! nothing here is a chat message.
//!
//! The ordering between them is a hard dependency in both directions. The client will not
//! accept a channel list until its IRC session has been greeted with numeric `001`, and
//! without the channel list it never issues a `JOIN`, so the IRC side sits registered and
//! silent. Both symptoms show up as a chat window that accepts input and shows nothing.
//!
//! Reversed in `documentations/chat-and-commands.md`, working end to end against build 10414.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// Counts and type indices are both written this wide.
const FIELD_BITS: u32 = ranged_bits(8);

/// Which tab a channel appears in.
///
/// The number is an **index into a pointer table of names inside the client**, so these are
/// not arbitrary labels: sending 3 for what the client calls `Guild` puts the messages in the
/// wrong tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    World,
    Team,
    Guild,
    PvP,
    Red,
    Blue,
    Help,
    Private,
    /// The "number of / none" sentinel. **The only type the client drops.**
    Noof,
}

impl ChannelType {
    pub const ALL: &'static [ChannelType] = &[
        ChannelType::World,
        ChannelType::Team,
        ChannelType::Guild,
        ChannelType::PvP,
        ChannelType::Red,
        ChannelType::Blue,
        ChannelType::Help,
        ChannelType::Private,
        ChannelType::Noof,
    ];

    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    /// Whether the client keeps a channel of this type.
    ///
    /// Every type except [`ChannelType::Noof`]. An earlier note in this project claimed only
    /// `World` and `PvP` were acted on; reading the consumer line by line, those two merely
    /// get an extra flag set, and everything but 8 is registered.
    pub fn is_registered(self) -> bool {
        self != Self::Noof
    }
}

/// One channel the client should know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub channel_type: ChannelType,

    /// **Without a leading `#`.** The client prefixes one itself, so `#global` becomes
    /// `##global` -- corrected by a live capture after an earlier claim to the contrary.
    pub name: String,

    /// Always set by the emulator; its meaning is unconfirmed.
    pub flag: bool,
}

impl Channel {
    /// Parse a `type:name,type:name` list, as the configuration is written.
    ///
    /// A bare name is the `World` channel. A malformed entry is **skipped**, not fatal: this
    /// is configuration a human typed, and one bad entry must not leave a player with no
    /// channels at all.
    pub fn parse_list(spec: &str) -> Vec<Channel> {
        spec.split(',')
            .filter_map(|entry| {
                let entry = entry.trim();

                if entry.is_empty() {
                    return None;
                }

                let (channel_type, name) = match entry.split_once(':') {
                    Some((index, name)) => {
                        (ChannelType::from_index(index.trim().parse().ok()?)?, name)
                    }

                    None => (ChannelType::World, entry),
                };

                let name = name.trim();

                (!name.is_empty()).then(|| Channel {
                    channel_type,
                    name: name.to_owned(),
                    flag: true,
                })
            })
            .collect()
    }
}

/// `RequestChatChannelData` -- "which channels are there?". No body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestChatChannelData;

impl RequestChatChannelData {
    pub const ID: u16 = 91;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);
    }

    pub fn decode(_reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self)
    }
}

/// `SendChatChannelData` -- the channel list.
///
/// ```text
/// count                28 bits
/// per channel:
///   type               28 bits
///   name               string, no leading '#'
///   flag                1 bit
/// trailing1            string
/// trailing2            string
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendChatChannelData {
    pub channels: Vec<Channel>,

    /// Two strings the client reads and never uses.
    ///
    /// Kept because it *reads* them: leaving them out desynchronises its reader rather than
    /// being skipped. Empty has always worked.
    pub trailing: [String; 2],
}

impl SendChatChannelData {
    pub const ID: u16 = 92;

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_packet_id(Self::ID);

        writer.write_bits_le(self.channels.len() as u32, FIELD_BITS);

        for channel in &self.channels {
            writer.write_bits_le(channel.channel_type.index(), FIELD_BITS);
            writer.write_string(&channel.name);
            writer.write_bit(channel.flag);
        }

        for trailing in &self.trailing {
            writer.write_string(trailing);
        }
    }
}
