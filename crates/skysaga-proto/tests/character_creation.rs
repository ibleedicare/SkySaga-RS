//! The character-creation packets, against captures from the real RakNet BitStream.
//!
//! The flow, from `documentations/character-and-appearance.md` §1:
//!
//! ```text
//! C->S SaveCharacterName            { name }                        108
//! S->C CharcterCreationResponse     { CharacterSaved }              109
//! C->S CreateHomeworld              { "Sky_Island", characterUUID } 110
//! S->C CharcterCreationResponse     { HomeworldCreated }            109
//! C->S SetCharacterCustomisationData{ entityId, customisation }      37
//! ```

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::customisation::{Attachment, CustomisationData, Gender};
use skysaga_proto::packets::{
    CharacterCreationResponse, CreateHomeworld, SaveCharacterName, SetCharacterCustomisationData,
};

mod golden;

use golden::{bytes_of, vector};

// --- SaveCharacterName (108, C->S) ---------------------------------------------------------

#[test]
fn save_character_name_decodes_the_capture() {
    let capture = vector("packet_save_character_name_Alice");
    let mut reader = BitReader::new(&capture.bytes, capture.bits);

    assert_eq!(reader.read_packet_id().unwrap(), SaveCharacterName::ID);

    let packet = SaveCharacterName::decode(&mut reader).unwrap();

    assert_eq!(packet.name, "Alice");
}

#[test]
fn save_character_name_re_encodes_the_capture() {
    let capture = vector("packet_save_character_name_Alice");

    let mut writer = BitWriter::new();
    writer.write_packet_id(SaveCharacterName::ID);

    SaveCharacterName {
        name: "Alice".to_owned(),
    }
    .encode(&mut writer);

    assert_eq!(bytes_of(&writer), capture.hex());
    assert_eq!(writer.bits_used(), capture.bits);
}

/// A truncated packet must be an error, not a panic.
#[test]
fn save_character_name_rejects_a_truncated_packet() {
    let capture = vector("packet_save_character_name_Alice");
    let mut reader = BitReader::new(&capture.bytes, 12);

    reader.read_packet_id().unwrap();

    assert!(SaveCharacterName::decode(&mut reader).is_err());
}

// --- CreateHomeworld (110, C->S) -----------------------------------------------------------

#[test]
fn create_homeworld_decodes_the_capture() {
    let capture = vector("packet_create_homeworld");
    let mut reader = BitReader::new(&capture.bytes, capture.bits);

    assert_eq!(reader.read_packet_id().unwrap(), CreateHomeworld::ID);

    let packet = CreateHomeworld::decode(&mut reader).unwrap();

    // "Sky_Island" is a geodata *Biome* name, not an island name.
    assert_eq!(packet.home_island_name, "Sky_Island");
    assert_eq!(packet.character_uuid, "8438a953-1a08-4959-9717-dff15d6e3574");
}

// --- CharcterCreationResponse (109, S->C) --------------------------------------------------
//
// The client's spelling of the packet is "Charcter"; the type is spelled correctly here and
// the wire id is what matters.

/// The one packet in this flow the *server* sends. Without it the creator hangs forever.
#[test]
fn every_creation_response_matches_its_capture() {
    let cases = [
        (CharacterCreationResponse::CharacterSaved, 0),
        (CharacterCreationResponse::CharacterSaveFailed, 1),
        (CharacterCreationResponse::HomeworldCreated, 2),
        (CharacterCreationResponse::HomeworldCreationFailed, 3),
    ];

    for (response, value) in cases {
        let capture = vector(&format!("packet_character_creation_response_{value}"));

        let mut writer = BitWriter::new();
        response.encode(&mut writer);

        assert_eq!(bytes_of(&writer), capture.hex(), "response {value}");
        assert_eq!(writer.bits_used(), capture.bits, "response {value}");
    }
}

/// 3 bits of payload after a one-byte id, so 11 bits — two bytes on the wire.
#[test]
fn a_creation_response_is_eleven_bits() {
    let mut writer = BitWriter::new();
    CharacterCreationResponse::CharacterSaved.encode(&mut writer);

    assert_eq!(writer.bits_used(), 11);
    assert_eq!(writer.as_bytes().len(), 2);
}

#[test]
fn creation_responses_round_trip() {
    for response in [
        CharacterCreationResponse::CharacterSaved,
        CharacterCreationResponse::CharacterSaveFailed,
        CharacterCreationResponse::HomeworldCreated,
        CharacterCreationResponse::HomeworldCreationFailed,
    ] {
        let mut writer = BitWriter::new();
        response.encode(&mut writer);

        let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

        assert_eq!(reader.read_packet_id().unwrap(), CharacterCreationResponse::ID);
        assert_eq!(CharacterCreationResponse::decode(&mut reader).unwrap(), response);
    }
}

// --- SetCharacterCustomisationData (37, C->S) ----------------------------------------------

fn populated() -> SetCharacterCustomisationData {
    SetCharacterCustomisationData {
        entity_id: 4242,
        customisation: CustomisationData {
            gender: Gender::Female,
            tribe: Some(1_319_509_738), // CRC32("human")
            materials: vec![Some(111), Some(222), Some(333)],
            attachments: vec![Attachment {
                attachment: Some(2_632_453_954), // CRC32("humanmalehairstyle01")
                material: Some(444),
            }],
        },
    }
}

#[test]
fn customisation_decodes_the_capture() {
    let capture = vector("packet_set_character_customisation");
    let mut reader = BitReader::new(&capture.bytes, capture.bits);

    assert_eq!(
        reader.read_packet_id().unwrap(),
        SetCharacterCustomisationData::ID
    );

    let packet = SetCharacterCustomisationData::decode(&mut reader).unwrap();

    assert_eq!(packet, populated());
}

#[test]
fn customisation_re_encodes_the_capture() {
    let capture = vector("packet_set_character_customisation");

    let mut writer = BitWriter::new();
    writer.write_packet_id(SetCharacterCustomisationData::ID);
    populated().encode(&mut writer);

    assert_eq!(bytes_of(&writer), capture.hex());
    assert_eq!(writer.bits_used(), capture.bits);
}

/// Everything absent — the optional-none path, end to end.
#[test]
fn an_empty_customisation_matches_its_capture() {
    let capture = vector("packet_set_character_customisation_empty");

    let empty = SetCharacterCustomisationData {
        entity_id: 0,
        customisation: CustomisationData {
            gender: Gender::Male,
            tribe: None,
            materials: vec![None, None, None],
            attachments: vec![Attachment {
                attachment: None,
                material: None,
            }],
        },
    };

    let mut writer = BitWriter::new();
    writer.write_packet_id(SetCharacterCustomisationData::ID);
    empty.encode(&mut writer);

    assert_eq!(bytes_of(&writer), capture.hex());

    let mut reader = BitReader::new(&capture.bytes, capture.bits);
    reader.read_packet_id().unwrap();

    assert_eq!(SetCharacterCustomisationData::decode(&mut reader).unwrap(), empty);
}

/// The escape path: the count field is genuinely zero bits wide, so a list of the default
/// length costs a single `0` bit, and any other length costs `1` plus a full 32-bit count.
/// The creator never sends this, but the schema allows it and the encoding must be right.
#[test]
fn a_non_default_attachment_count_uses_the_escape_path() {
    let capture = vector("packet_customisation_two_attachments");

    let data = CustomisationData {
        gender: Gender::Male,
        tribe: None,
        materials: vec![None, None, None],
        attachments: vec![
            Attachment {
                attachment: Some(1),
                material: Some(2),
            },
            Attachment {
                attachment: Some(3),
                material: Some(4),
            },
        ],
    };

    let mut writer = BitWriter::new();
    data.encode(&mut writer);

    assert_eq!(bytes_of(&writer), capture.hex());

    let mut reader = BitReader::new(&capture.bytes, capture.bits);

    assert_eq!(CustomisationData::decode(&mut reader).unwrap(), data);
}

/// A default-length list must cost exactly one bit more than its elements — if this grows,
/// the zero-width count has been mistaken for a real one.
#[test]
fn a_default_length_list_costs_one_bit() {
    let mut with_default = BitWriter::new();

    CustomisationData {
        gender: Gender::Male,
        tribe: None,
        materials: vec![None, None, None],
        attachments: vec![Attachment {
            attachment: None,
            material: None,
        }],
    }
    .encode(&mut with_default);

    // gender 2 + tribe 1 + (escape 1 + 3 absent materials) + (escape 1 + 2 absent optionals)
    //   = 2 + 1 + 4 + 3 = 10
    // Cross-checked against the capture: packet_set_character_customisation_empty is 50 bits,
    // which is 8 (packet id) + 32 (entityId) + 10.
    assert_eq!(with_default.bits_used(), 10);
}

// --- the appearance accessors ---------------------------------------------------------------

/// `materials` is positional — skin, eyes, clothing — and `attachments[0]` is the hair.
/// Named accessors so callers do not index by magic number.
#[test]
fn named_accessors_map_onto_the_positional_slots() {
    let data = populated().customisation;

    assert_eq!(data.skin(), Some(111));
    assert_eq!(data.eyes(), Some(222));
    assert_eq!(data.clothing(), Some(333));
    assert_eq!(data.hair_style(), Some(2_632_453_954));
    assert_eq!(data.hair_colour(), Some(444));
}

#[test]
fn accessors_are_none_when_the_slots_are_missing() {
    let data = CustomisationData::default();

    assert_eq!(data.skin(), None);
    assert_eq!(data.eyes(), None);
    assert_eq!(data.clothing(), None);
    assert_eq!(data.hair_style(), None);
    assert_eq!(data.hair_colour(), None);
}

/// The default must be wire-legal: three material slots and one attachment, matching the
/// list defaults, or the very first packet sent is malformed.
#[test]
fn the_default_has_the_schema_default_lengths() {
    let data = CustomisationData::default();

    assert_eq!(data.materials.len(), 3);
    assert_eq!(data.attachments.len(), 1);
    assert_eq!(data.gender, Gender::Male);

    let mut writer = BitWriter::new();
    data.encode(&mut writer);

    assert_eq!(writer.bits_used(), 10, "the cheapest legal encoding");
}

// --- gender ----------------------------------------------------------------------------------

/// 2 bits wide even though only two values exist — `NumBitsRequired(2)` declares a max of 2.
/// An unknown third value must decode rather than fail, since the client could send it.
#[test]
fn gender_is_two_bits_and_tolerates_an_unknown_value() {
    assert_eq!(Gender::Male.value(), 0);
    assert_eq!(Gender::Female.value(), 1);

    let mut writer = BitWriter::new();
    writer.write_bits_le(2, 2);

    let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

    assert_eq!(Gender::decode(&mut reader).unwrap(), Gender::Unknown(2));
}

#[test]
fn gender_round_trips() {
    for gender in [Gender::Male, Gender::Female, Gender::Unknown(2)] {
        let mut writer = BitWriter::new();
        gender.encode(&mut writer);

        assert_eq!(writer.bits_used(), 2);

        let mut reader = BitReader::new(writer.as_bytes(), writer.bits_used());

        assert_eq!(Gender::decode(&mut reader).unwrap(), gender);
    }
}

// --- name hashing ------------------------------------------------------------------------------

/// Every id in the struct is `CRC32(name.to_lowercase())` — the same hash the rest of the
/// protocol uses for entity, item and currency names.
#[test]
fn the_geodata_names_hash_to_the_documented_ids() {
    use skysaga_core::name_hash;

    assert_eq!(name_hash("Cat"), 253_473_828);
    assert_eq!(name_hash("Human"), 1_319_509_738);
    assert_eq!(name_hash("Lizard"), 2_876_448_639);
    assert_eq!(name_hash("HumanMaleHairstyle01"), 2_632_453_954);
}
