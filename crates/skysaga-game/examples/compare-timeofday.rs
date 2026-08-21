//! Compare the TimeOfDay we send with the one the C# sent.
use skysaga_game::{World, WorldConfig};
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::{EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions, TimeOfDayComponent};

fn decode(entity: &EntityAdd, count: usize) -> TimeOfDayComponent {
    let mut r = BitReader::new(entity.sync_data.bytes(), entity.sync_data.len());
    let sync = SyncData::decode(&mut r, count).unwrap();
    let mut p = BitReader::new(sync.parameters.bytes(), sync.parameters.len());
    TimeOfDayComponent::decode_all(&mut p).unwrap()
}

fn main() {
    let definitions = EntityDefinitions::load(default_entities_path()).unwrap();
    let tod = definitions.get("TimeOfDay").unwrap();
    let count = tod.synced_parameter_count();

    let world = World::home_island(&definitions, &WorldConfig::default());
    let ours = world
        .entities
        .iter()
        .find(|e| e.name_hash == Some(tod.name_hash()))
        .unwrap();

    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../skysaga-proto/tests/fixtures/handshake.tsv"
    ))
    .unwrap();

    for line in text.lines().filter(|l| l.starts_with("server_234_")) {
        let f: Vec<&str> = line.split('\t').collect();
        let bytes: Vec<u8> = (0..f[2].len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&f[2][i..i + 2], 16).unwrap())
            .collect();
        let mut r = BitReader::from_bytes(&bytes);
        r.read_packet_id().unwrap();
        let e = EntityAdd::decode(&mut r).unwrap();

        if e.name_hash == Some(tod.name_hash()) {
            println!("C#:   {:#?}", decode(&e, count));
            println!("Rust: {:#?}", decode(ours, count));
        }
    }
}
