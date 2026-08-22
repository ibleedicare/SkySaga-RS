//! Decode the captured `BasicInventoryItem` entities field by field.
//!
//! The C# server's own loadout items, off the wire. This says what a real item looks like,
//! rather than what reading the serialiser suggests it should.
//!
//!   cargo run -p skysaga-world --example decode-item

use skysaga_proto::bitstream::{BitReader, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions};

const CAPTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../skysaga-proto/tests/fixtures/handshake.tsv"
);

fn main() {
    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let item = definitions.get("BasicInventoryItem").expect("defined");
    let text = std::fs::read_to_string(CAPTURE).expect("capture");

    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        let bytes = from_hex(fields[2].trim());

        let mut reader = BitReader::from_bytes(&bytes);

        let Ok(id) = reader.read_packet_id() else { continue };

        if id + ID_USER_PACKET_ENUM != 234 {
            continue;
        }

        let Ok(add) = EntityAdd::decode(&mut reader) else { continue };

        if add.name_hash != Some(item.name_hash()) {
            continue;
        }

        println!("entity {} (name_hash {:?}, parent {:?})", add.id, add.name_hash, add.parent_id);

        let mut sync = BitReader::from_bytes(add.sync_data.bytes());
        let data = SyncData::decode(&mut sync, item.synced_parameter_count()).expect("sync");

        println!("  flags: {:?}", data.present);
        println!("  payload: {} bits", data.parameters.len());

        // Parameters in sync-index order: 0 bool, 1 bool, 2 slot data, 3 bool.
        let mut p = BitReader::from_bytes(data.parameters.bytes());

        println!("  0 allowaddingtofoundinbiomes = {:?}", p.read_bit());
        println!("  1 hasbeentransferred         = {:?}", p.read_bit());

        println!("  -- inventoryslotdata --");
        let has_name = p.read_bit().unwrap();
        let name = if has_name { Some(p.read_u32().unwrap()) } else { None };
        println!("     name          = {name:?} (present {has_name})");

        let large = p.read_bit().unwrap();
        let count = p.read_bits_le(if large { 17 } else { 7 }).unwrap();
        println!("     count         = {count} (large {large})");

        println!("     unknown3      = {:?}", p.read_bit());

        let large4 = p.read_bit().unwrap();
        let unknown4 = p.read_bits_le(if large4 { 17 } else { 7 }).unwrap();
        println!("     unknown4      = {unknown4} (large {large4})");

        println!("     unknown5      = {:?}", p.read_bits_le(17));
        println!("     item uuid     = {:?}", p.read_string());

        println!("  3 itemlocked                 = {:?}", p.read_bit());
        println!();
    }
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}
