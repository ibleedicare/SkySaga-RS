//! Decode a live capture both ways and resolve the ids against real GeoData names.
//!
//! The client is the final oracle for byte order: whichever interpretation makes the tribe,
//! material and attachment ids resolve to actual names in geodata.json is the correct one.
//!
//!   cargo run -p skysaga-proto --example resolve-capture -- <geodata.json> <hex> [<hex>...]

use std::collections::HashMap;

use skysaga_core::name_hash;

/// Pull the `"Name": "..."` values out of the geodata sections that matter, without a JSON
/// dependency -- every id in CustomisationData is CRC32 of one of these.
fn names(geodata: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();

    for (index, _) in geodata.match_indices("\"Name\"") {
        let rest = &geodata[index + 6..];

        let Some(open) = rest.find('"') else { continue };
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { continue };

        let name = &after[..close];

        if !name.is_empty() && name.len() < 64 {
            out.insert(name_hash(name), name.to_owned());
        }
    }

    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("geodata.json path");
    let geodata = std::fs::read_to_string(&path).expect("geodata readable");
    let names = names(&geodata);

    println!("{} distinct names in {path}\n", names.len());

    for hex in args {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect();

        println!("--- {} bytes", bytes.len());

        for big_endian in [false, true] {
            let label = if big_endian { "big-endian" } else { "little-endian" };

            // Hand-rolled walk so the byte order can be flipped per field.
            let mut reader = Bits {
                data: &bytes,
                offset: 8, // skip the one-byte packet id
                big_endian,
            };

            let entity = reader.u32();
            let gender = reader.bits(2);
            let tribe = reader.optional();
            let _material_escape = reader.bit();
            let materials: Vec<_> = (0..3).map(|_| reader.optional()).collect();
            let _attachment_escape = reader.bit();
            let hair_style = reader.optional();
            let hair_colour = reader.optional();

            let resolve = |id: Option<u32>| match id {
                None => "-".to_owned(),
                Some(id) => names
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("?{id}")),
            };

            let fields = [tribe, materials[0], materials[1], materials[2], hair_style, hair_colour];
            let hits = fields.iter().filter(|id| id.is_some_and(|id| names.contains_key(&id))).count();

            println!("  {label:14} entity {entity:<12} gender {gender}  resolved {hits}/6");
            println!("      tribe    {}", resolve(tribe));
            println!("      skin     {}", resolve(materials[0]));
            println!("      eyes     {}", resolve(materials[1]));
            println!("      clothing {}", resolve(materials[2]));
            println!("      hair     {} / {}", resolve(hair_style), resolve(hair_colour));
        }

        println!();
    }
}

struct Bits<'a> {
    data: &'a [u8],
    offset: usize,
    big_endian: bool,
}

impl Bits<'_> {
    fn bit(&mut self) -> bool {
        if self.offset >= self.data.len() * 8 {
            return false;
        }

        let bit = self.data[self.offset / 8] & (0x80 >> (self.offset % 8)) != 0;
        self.offset += 1;
        bit
    }

    fn bits(&mut self, count: u32) -> u32 {
        let mut value = 0;

        for _ in 0..count {
            value = (value << 1) | u32::from(self.bit());
        }

        value
    }

    fn u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];

        for byte in &mut bytes {
            *byte = self.bits(8) as u8;
        }

        if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    }

    fn optional(&mut self) -> Option<u32> {
        if self.bit() {
            Some(self.u32())
        } else {
            None
        }
    }
}
