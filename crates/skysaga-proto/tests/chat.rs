//! The two RakNet chat packets.
//!
//! They carry the **list of channels and nothing else**. Every actual message travels over a
//! separate TCP socket speaking IRC, so nothing here is a chat message -- these are what tells
//! the client which channels exist so it knows what to `JOIN`.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::chat::{Channel, ChannelType, RequestChatChannelData, SendChatChannelData};

fn encoded(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

#[test]
fn a_channel_request_is_one_byte() {
    // The handler ignores the stream entirely; the id is the whole message.
    let bytes = encoded(|w| RequestChatChannelData.encode(w));

    assert_eq!(bytes.len(), 1);

    let mut reader = BitReader::from_bytes(&bytes);
    assert_eq!(reader.read_packet_id().unwrap(), 91);
}

#[test]
fn the_channel_list_is_written_in_the_order_the_client_reads_it() {
    let bytes = encoded(|w| {
        SendChatChannelData {
            channels: vec![Channel {
                channel_type: ChannelType::World,
                name: "global".to_owned(),
                flag: true,
            }],
            trailing: [String::new(), String::new()],
        }
        .encode(w)
    });

    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_packet_id().unwrap(), 92);

    // count and type are both `32 - NumBitsRequired(8)` bits.
    //
    // That is **4**, not 28: `NumBitsRequired` returns the leading-zero count, so for 8 it is
    // 28 and the width is 32 - 28. `documentations/chat-and-commands.md` said 28 in its table,
    // which is the arithmetic done the wrong way round; the C# serializer computes 4 and this
    // test was written from the doc before being checked against it.
    assert_eq!(reader.read_bits_le(4).unwrap(), 1, "one channel");
    assert_eq!(reader.read_bits_le(4).unwrap(), 0, "World");
    assert_eq!(reader.read_string().unwrap(), "global");
    assert!(reader.read_bit().unwrap(), "the flag");

    // Two trailing strings the client reads and never uses. Absent ones desynchronise its
    // reader rather than being skipped.
    assert_eq!(reader.read_string().unwrap(), "");
    assert_eq!(reader.read_string().unwrap(), "");
}

#[test]
fn the_channel_name_carries_no_leading_hash() {
    // **The client prefixes `#` itself.** Sending `#global` yields `##global`, which a live
    // capture on 2026-08-19 corrected an earlier claim about.
    let bytes = encoded(|w| {
        SendChatChannelData {
            channels: vec![Channel {
                channel_type: ChannelType::World,
                name: "global".to_owned(),
                flag: true,
            }],
            trailing: [String::new(), String::new()],
        }
        .encode(w)
    });

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();
    reader.skip_bits(4 * 2).unwrap();

    let name = reader.read_string().unwrap();

    assert!(!name.starts_with('#'), "the client would send ##{name}");
}

#[test]
fn several_channels_round_trip() {
    let packet = SendChatChannelData {
        channels: vec![
            Channel {
                channel_type: ChannelType::World,
                name: "global".to_owned(),
                flag: true,
            },
            Channel {
                channel_type: ChannelType::PvP,
                name: "local".to_owned(),
                flag: true,
            },
        ],
        trailing: [String::new(), String::new()],
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut reader = BitReader::from_bytes(&bytes);
    reader.read_packet_id().unwrap();

    assert_eq!(reader.read_bits_le(4).unwrap(), 2);
}

#[test]
fn channel_types_have_the_numbers_the_client_indexes_by() {
    // The type is an index into a pointer table of names in the client, so these are not
    // arbitrary: sending 3 for what the client calls Guild puts the messages in the wrong tab.
    assert_eq!(ChannelType::World.index(), 0);
    assert_eq!(ChannelType::Team.index(), 1);
    assert_eq!(ChannelType::Guild.index(), 2);
    assert_eq!(ChannelType::PvP.index(), 3);
    assert_eq!(ChannelType::Help.index(), 6);
    assert_eq!(ChannelType::Private.index(), 7);
    assert_eq!(ChannelType::Noof.index(), 8);
}

#[test]
fn the_none_sentinel_is_the_only_type_the_client_drops() {
    // Reading the consumer line by line: every type except 8 is registered. An earlier note
    // in this project claimed only 0 and 3 were acted on, which is not what it does -- 0 and 3
    // merely get an extra flag set.
    assert!(!ChannelType::Noof.is_registered());

    for channel_type in ChannelType::ALL {
        if *channel_type != ChannelType::Noof {
            assert!(channel_type.is_registered(), "{channel_type:?}");
        }
    }
}

#[test]
fn a_channel_spec_parses_the_way_the_environment_variable_is_written() {
    assert_eq!(
        Channel::parse_list("0:global,3:local"),
        vec![
            Channel {
                channel_type: ChannelType::World,
                name: "global".to_owned(),
                flag: true,
            },
            Channel {
                channel_type: ChannelType::PvP,
                name: "local".to_owned(),
                flag: true,
            },
        ],
    );
}

#[test]
fn a_bare_name_defaults_to_the_world_channel() {
    assert_eq!(
        Channel::parse_list("global"),
        vec![Channel {
            channel_type: ChannelType::World,
            name: "global".to_owned(),
            flag: true,
        }],
    );
}

#[test]
fn a_malformed_spec_is_skipped_rather_than_taking_the_list_down() {
    // The value is configuration a human typed. One bad entry must not leave a player with no
    // channels at all, which is a chat window that accepts input and shows nothing.
    assert_eq!(
        Channel::parse_list("0:global,,nonsense:x,3:local"),
        Channel::parse_list("0:global,3:local"),
    );
}
