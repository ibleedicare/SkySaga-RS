//! Print what the captured ChunkSync packets actually contain.
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::ChunkSync;

fn main() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/handshake.tsv"
    ))
    .unwrap();

    for line in text.lines().filter(|l| l.starts_with("server_142_")) {
        let f: Vec<&str> = line.split('\t').collect();
        let bytes: Vec<u8> = (0..f[2].len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&f[2][i..i + 2], 16).unwrap())
            .collect();

        let mut r = BitReader::from_bytes(&bytes);
        r.read_packet_id().unwrap();
        let c = ChunkSync::decode(&mut r).unwrap();

        println!(
            "{:<14} coords {:?}  data1 {:?}  data2 {:?}  adjacent {:?}",
            f[0],
            c.coords,
            c.data1.as_ref().map(|d| d.len()),
            c.data2.as_ref().map(|d| d.len()),
            c.adjacent_chunks
        );
    }
}
