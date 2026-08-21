//! What position did the C# actually give the Tree?
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::{EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions, TransformComponent};

fn main() {
    let definitions = EntityDefinitions::load(default_entities_path()).unwrap();
    let tree = definitions.get("Tree").unwrap();

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

        if e.name_hash != Some(tree.name_hash()) {
            continue;
        }

        let mut sr = BitReader::new(e.sync_data.bytes(), e.sync_data.len());
        let sync = SyncData::decode(&mut sr, tree.synced_parameter_count()).unwrap();
        let mut pr = BitReader::new(sync.parameters.bytes(), sync.parameters.len());

        println!("Tree position = {:?}", TransformComponent::read_position(&mut pr).unwrap());
        println!("Tree size     = {:?}", TransformComponent::read_size(&mut pr).unwrap());
    }
}
