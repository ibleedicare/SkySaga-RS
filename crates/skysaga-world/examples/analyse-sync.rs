//! Decode the captured EntityAdd packets and report which sync indices each entity set.
//!
//! Ground truth for the component work: rather than reading the C# and hoping, this says
//! exactly which (component, parameter) pairs appear in a real handshake and how many bits
//! the whole payload took.
//!
//!   cargo run -p skysaga-world --example analyse-sync

use skysaga_proto::bitstream::{BitReader, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions};

const CAPTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../skysaga-proto/tests/fixtures/handshake.tsv"
);

fn main() {
    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let text = std::fs::read_to_string(CAPTURE).expect("capture");

    let mut needed: std::collections::BTreeMap<String, usize> = Default::default();

    for line in text.lines().filter(|l| l.starts_with("server_234_")) {
        let fields: Vec<&str> = line.split('\t').collect();
        let bytes: Vec<u8> = (0..fields[2].len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&fields[2][i..i + 2], 16).unwrap())
            .collect();

        let mut reader = BitReader::from_bytes(&bytes);
        let id = reader.read_packet_id().unwrap();

        assert_eq!(id + ID_USER_PACKET_ENUM, 234);

        let packet = EntityAdd::decode(&mut reader).unwrap();
        let hash = packet.name_hash.unwrap_or(0);

        let definition = definitions.iter().find(|d| d.name_hash() == hash);

        let Some(definition) = definition else {
            println!("entity {:<4} hash {hash:#010x}  UNKNOWN", packet.id);
            continue;
        };

        let mut sync_reader =
            BitReader::new(packet.sync_data.bytes(), packet.sync_data.len());

        let sync = match SyncData::decode(&mut sync_reader, definition.synced_parameter_count()) {
            Ok(sync) => sync,
            Err(error) => {
                println!(
                    "entity {:<4} {:<16} FAILED to parse with {} flags: {error}",
                    packet.id,
                    definition.name(),
                    definition.synced_parameter_count(),
                );
                continue;
            }
        };

        let set: Vec<usize> = sync.present_indices().collect();

        println!(
            "entity {:<4} {:<16} {} flags, {} set, {} payload bits",
            packet.id,
            definition.name(),
            definition.synced_parameter_count(),
            set.len(),
            sync.parameters.len(),
        );

        for index in set {
            match definition.parameter_at(index) {
                Some((component, parameter)) => {
                    println!("    {index:>3}  {component} :: {parameter}");
                    *needed.entry(component.to_owned()).or_default() += 1;
                }
                None => println!("    {index:>3}  <unresolved>"),
            }
        }
    }

    println!("\n=== components to implement, by how often they appear ===");

    let mut by_count: Vec<_> = needed.into_iter().collect();
    by_count.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (component, count) in by_count {
        println!("  {count:>3}x  {component}");
    }
}
