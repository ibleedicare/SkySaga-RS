//! Decode a live `SaveCharacterName` capture.
//!
//!   cargo run -p skysaga-proto --example decode-name -- F283DC...

use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::SaveCharacterName;

fn main() {
    for hex in std::env::args().skip(1) {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect();

        let mut reader = BitReader::from_bytes(&bytes);
        let id = reader.read_packet_id();

        println!("{} bytes, packet id {id:?}", bytes.len());
        println!("  {:?}", SaveCharacterName::decode(&mut reader));
        println!("  bits read {} of {}", reader.bits_read(), bytes.len() * 8);
    }
}
