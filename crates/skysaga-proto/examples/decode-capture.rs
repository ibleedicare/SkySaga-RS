//! Decode a live `SetCharacterCustomisationData` capture taken from the running client.
//!
//!   cargo run -p skysaga-proto --example decode-capture -- AB0000...
//!
//! The C# game server prints these as `[warn] unhandled packet ... {hex}`.

use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::SetCharacterCustomisationData;

fn main() {
    for hex in std::env::args().skip(1) {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect();

        println!("--- {} bytes", bytes.len());

        let mut reader = BitReader::from_bytes(&bytes);

        match reader.read_packet_id() {
            Ok(id) if id == SetCharacterCustomisationData::ID => {}
            other => {
                println!("  not a customisation packet: {other:?}");
                continue;
            }
        }

        match SetCharacterCustomisationData::decode(&mut reader) {
            Ok(packet) => {
                let c = &packet.customisation;

                println!("  entity_id   {}", packet.entity_id);
                println!("  gender      {:?}", c.gender);
                println!("  tribe       {:?}", c.tribe);
                println!("  skin        {:?}", c.skin());
                println!("  eyes        {:?}", c.eyes());
                println!("  clothing    {:?}", c.clothing());
                println!("  hair style  {:?}", c.hair_style());
                println!("  hair colour {:?}", c.hair_colour());
                println!("  materials {} attachments {}", c.materials.len(), c.attachments.len());
                println!("  bits read {} of {}", reader.bits_read(), bytes.len() * 8);
            }
            Err(error) => println!("  decode failed: {error}"),
        }
    }
}
