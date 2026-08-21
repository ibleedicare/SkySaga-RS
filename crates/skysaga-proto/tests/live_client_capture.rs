//! Decoding real packets captured from the running client.
//!
//! Everything else in this crate is checked against the C# `BitStream`, which is a good
//! oracle for *how RakNet serialises* but a poor one for *what the client actually sends* —
//! the generator only reproduces the shape it is told to write. These captures close that
//! gap: they came off the wire from a live SkySaga client (build 10414) in its character
//! creator, logged by the C# game server as `[warn] unhandled packet ... {hex}`.
//!
//! They are what caught the byte-order defect. The 32-bit fields were originally written with
//! the emulator's little-endian `WriteBits(GetBytes(v), 32, true)` idiom; the C# agreed,
//! because that is what the generator asked it for. The client did not.

use skysaga_core::name_hash;
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::customisation::Gender;
use skysaga_proto::packets::SetCharacterCustomisationData;

/// `SetCharacterCustomisationData`, sent live by the client's character creator as the
/// player changes appearance options. Two different appearances, same session.
const CAPTURES: &[&str] = &[
    "AB0000000C69D4C3DD4B968327F6DEAB717ADDDAD8AAA473C10FC08E36ADC0",
    "AB0000000C61E376848A0F61AE0428E6F82EB81B087A952CB94C649A5701C0",
];

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn decode(hex: &str) -> SetCharacterCustomisationData {
    let bytes = bytes(hex);
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(
        reader.read_packet_id().unwrap(),
        SetCharacterCustomisationData::ID,
        "wire id 0xAB is packet 37",
    );

    SetCharacterCustomisationData::decode(&mut reader).expect("live capture decodes")
}

/// The whole packet parses, and consumes the bits it should.
#[test]
fn a_live_capture_decodes_completely() {
    for hex in CAPTURES {
        let bytes = bytes(hex);
        let mut reader = BitReader::from_bytes(&bytes);

        reader.read_packet_id().unwrap();
        SetCharacterCustomisationData::decode(&mut reader).expect("decodes");

        // 8 id + 32 entity + 242-8-32 struct = 242 bits, in a 31-byte packet. Whatever is
        // left is RakNet's padding to the byte boundary, so under 8 bits.
        assert_eq!(reader.bits_read(), 242, "{hex}");
        assert!(reader.bits_remaining() < 8, "{hex}");
    }
}

/// The decisive one. Every id in the struct is `CRC32(geodata name)`, so if the byte order is
/// wrong the values are noise. Read correctly, all six resolve to real names — and to a
/// *coherent* character, with the tribe agreeing with its own skin, eyes and hairstyle.
#[test]
fn a_live_capture_resolves_to_real_geodata_names() {
    let packet = decode(CAPTURES[0]);
    let c = &packet.customisation;

    assert_eq!(packet.entity_id, 12, "a small entity id, not 0x0C000000");
    assert_eq!(c.gender, Gender::Female);

    assert_eq!(c.tribe, Some(name_hash("Human")));
    assert_eq!(c.skin(), Some(name_hash("Human_Skin_Mid1")));
    assert_eq!(c.eyes(), Some(name_hash("Human_Eyes_Hazel")));
    assert_eq!(c.clothing(), Some(name_hash("Blue_Fabric")));
    assert_eq!(c.hair_style(), Some(name_hash("HumanFemaleHairstyle01")));
    assert_eq!(c.hair_colour(), Some(name_hash("Human_Hair_Grey")));
}

/// The list lengths the schema defaults to, straight off the wire — confirming the zero-width
/// count and its escape bit are read the way the client writes them.
#[test]
fn a_live_capture_uses_the_default_list_lengths() {
    for hex in CAPTURES {
        let c = decode(hex).customisation;

        assert_eq!(c.materials.len(), 3, "skin, eyes, clothing");
        assert_eq!(c.attachments.len(), 1, "the hair");
    }
}

/// Re-encoding a live capture must reproduce it bit for bit. This is the strongest statement
/// available without a transport: what the server would send is what the client sent.
#[test]
fn a_live_capture_re_encodes_to_the_same_bytes() {
    for hex in CAPTURES {
        let packet = decode(hex);

        let mut writer = skysaga_proto::bitstream::BitWriter::new();
        writer.write_packet_id(SetCharacterCustomisationData::ID);
        packet.encode(&mut writer);

        let original = bytes(hex);
        let encoded = writer.as_bytes();

        assert_eq!(writer.bits_used(), 242, "{hex}");

        // Compare only the bits the packet occupies; the capture's final byte carries
        // RakNet's padding, which is not ours to reproduce.
        let whole = writer.bits_used() / 8;

        assert_eq!(&encoded[..whole], &original[..whole], "{hex}");
    }
}

/// Both captures are the same character with different colours chosen, so the fields that
/// identify *who* it is stay put while the materials change. Guards against a decode that
/// happens to work on one sample by accident.
#[test]
fn two_captures_from_one_session_agree_on_identity() {
    let first = decode(CAPTURES[0]).customisation;
    let second = decode(CAPTURES[1]).customisation;

    assert_eq!(first.gender, second.gender);
    assert_ne!(
        (first.skin(), first.eyes(), first.hair_colour()),
        (second.skin(), second.eyes(), second.hair_colour()),
        "the player changed something between the two",
    );
}

// --- SaveCharacterName -----------------------------------------------------------------------

/// Captured from the same session, when the creator submitted the name. Wire id `0xF2` is
/// packet 108. The name is the account name because `nix run .#sky-saga` passes no `charname`
/// launch variable and the creator pre-fills it.
const SAVE_CHARACTER_NAME: &str = "F283DC1C9BDA9958DD1D8B58DB1A595B9D00";

#[test]
fn the_live_save_character_name_decodes() {
    use skysaga_proto::packets::SaveCharacterName;

    let bytes = bytes(SAVE_CHARACTER_NAME);
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(reader.read_packet_id().unwrap(), SaveCharacterName::ID);

    let packet = SaveCharacterName::decode(&mut reader).expect("live capture decodes");

    assert_eq!(packet.name, "projectv-client");

    // 8 id + 1 hasData + 1 largeLength + 8 length + 15*8 name = 138 bits.
    assert_eq!(reader.bits_read(), 138);
    assert!(reader.bits_remaining() < 8, "only RakNet's padding is left");
}

#[test]
fn the_live_save_character_name_re_encodes_to_the_same_bytes() {
    use skysaga_proto::bitstream::BitWriter;
    use skysaga_proto::packets::SaveCharacterName;

    let mut writer = BitWriter::new();
    writer.write_packet_id(SaveCharacterName::ID);

    SaveCharacterName {
        name: "projectv-client".to_owned(),
    }
    .encode(&mut writer);

    let original = bytes(SAVE_CHARACTER_NAME);
    let whole = writer.bits_used() / 8;

    assert_eq!(writer.bits_used(), 138);
    assert_eq!(&writer.as_bytes()[..whole], &original[..whole]);
}
