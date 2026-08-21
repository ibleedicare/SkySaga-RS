//! The photo packets, and why character creation depends on them.
//!
//! The client captures an avatar photo as the last step of character creation, sends
//! `NotifyPhotoCaptured`, and then **waits**: the capture sits in its pending queue until a
//! `PhotoValidated` echoes the id back with an upload token. Until that arrives the client
//! never uploads and never leaves `GameState_CharacterCreation` — observed against this
//! server as an indefinite stall on the "Character Creation" loading screen, with the client
//! log reading `Photo capture started: 1 waiting in total` and no matching "Exiting".
//!
//! Layouts from the C#'s own reversing notes (`Packets/NotifyPhotoCaptured.cs`, from the
//! client's serialiser `FUN_00791e60`, and `Packets/PhotoValidated.cs` from `FUN_0073f880`).

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::{NotifyPhotoCaptured, PhotoValidated};

/// Encode a capture the way the client does, so decoding is tested against a producer that
/// was written independently of the decoder.
fn captured(photo_id: u32, is_avatar: bool) -> Vec<u8> {
    let mut writer = BitWriter::new();

    writer.write_compressed_u32(photo_id);

    // position and direction: three floats each. Not needed to validate, but they are in
    // front of the avatar flag, so the decoder has to step over exactly 192 bits.
    for _ in 0..6 {
        writer.write_u32(0);
    }

    writer.write_bit(is_avatar);

    writer.into_bytes()
}

#[test]
fn a_capture_decodes_its_photo_id() {
    let bytes = captured(7, true);
    let mut reader = BitReader::from_bytes(&bytes);

    let packet = NotifyPhotoCaptured::decode(&mut reader).expect("decodes");

    assert_eq!(packet.client_photo_id, 7);
    assert!(packet.is_avatar_photo);
}

/// The avatar flag is what distinguishes the character-creation portrait from an in-game
/// snapshot, and it sits behind 192 bits of position and direction. Misreading those would
/// leave the flag wrong while the id still looked right.
#[test]
fn a_non_avatar_capture_is_distinguished() {
    let bytes = captured(7, false);
    let mut reader = BitReader::from_bytes(&bytes);

    let packet = NotifyPhotoCaptured::decode(&mut reader).expect("decodes");

    assert!(!packet.is_avatar_photo);
}

/// Ids above the short-form boundary must survive, or the reply echoes the wrong id and the
/// client never matches it to its pending capture.
#[test]
fn a_large_photo_id_survives() {
    let bytes = captured(0x1234, true);
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(
        NotifyPhotoCaptured::decode(&mut reader).unwrap().client_photo_id,
        0x1234,
    );
}

/// A capture carrying only its id must still decode.
///
/// This is the contract that matters. The server needs nothing but `clientPhotoID` to reply,
/// and the C# reads nothing else either. A live client's capture turned out to be shorter
/// than six raw floats -- RakNet may write the vectors compressed -- so a decoder that
/// insisted on the full layout errored, fell back to `Unknown`, sent no reply, and left the
/// client stuck in character creation. Being strict about a field nobody reads is worse than
/// useless here.
#[test]
fn a_capture_with_only_an_id_still_decodes() {
    let mut writer = BitWriter::new();
    writer.write_compressed_u32(9);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    let packet = NotifyPhotoCaptured::decode(&mut reader).expect("the id alone is enough");

    assert_eq!(packet.client_photo_id, 9);
    assert!(!packet.is_avatar_photo, "unknown, so not claimed to be an avatar");
}

/// An empty packet has no id at all, so it is still an error rather than a panic.
#[test]
fn an_empty_capture_is_refused() {
    let mut reader = BitReader::from_bytes(&[]);

    assert!(NotifyPhotoCaptured::decode(&mut reader).is_err());
}

/// The validation round-trips: id echoed, and both strings intact.
#[test]
fn a_validation_round_trips() {
    let validated = PhotoValidated {
        client_photo_id: 7,
        official_uuid: "3e195905-a077-48ab-9310-53df2276a402".to_owned(),
        upload_token: "d6422039-9778-468f-95f6-5390b72eb3b4".to_owned(),
    };

    let mut writer = BitWriter::new();
    validated.encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(
        reader.read_packet_id().expect("an id"),
        PhotoValidated::ID,
        "the reply must carry its own packet id",
    );

    assert_eq!(reader.read_compressed_u32().unwrap(), 7);
    assert_eq!(reader.read_string().unwrap(), validated.official_uuid);
    assert_eq!(reader.read_string().unwrap(), validated.upload_token);
}

/// The ordinals, which are what the unhandled-packet warnings report.
#[test]
fn the_packet_ids_match_the_client_table() {
    assert_eq!(NotifyPhotoCaptured::ID, 150);
    assert_eq!(PhotoValidated::ID, 152);
}
