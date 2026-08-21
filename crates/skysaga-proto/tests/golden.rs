//! Loads the RakNet captures in `tests/fixtures/bitstream.tsv`.
//!
//! Those vectors were produced by running the real `libRakNet.so` through
//! `tools/bitstream-golden`. They are the oracle for everything in this crate: the pure-Rust
//! BitStream is correct exactly when it reproduces them.
//!
//! Format: `label<TAB>bits<TAB>hex`, with `#` comments.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::HashMap;
use std::sync::OnceLock;

use skysaga_proto::bitstream::BitWriter;

pub struct Vector {
    pub bits: usize,
    pub bytes: Vec<u8>,
}

impl Vector {
    pub fn hex(&self) -> String {
        to_hex(&self.bytes)
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn bytes_of(writer: &BitWriter) -> String {
    to_hex(writer.as_bytes())
}

pub fn vector(label: &str) -> &'static Vector {
    vectors()
        .get(label)
        .unwrap_or_else(|| panic!("no golden vector named {label:?}"))
}

pub fn vectors() -> &'static HashMap<String, Vector> {
    static VECTORS: OnceLock<HashMap<String, Vector>> = OnceLock::new();

    VECTORS.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bitstream.tsv");
        let text = std::fs::read_to_string(path).expect("golden vectors are present");

        text.lines()
            .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                // Exactly three fields. Anything else means the generator's output was
                // polluted -- the nix shell banner goes to stdout, for instance -- and a
                // silently dropped vector would look like a passing test suite.
                let fields: Vec<&str> = line.split('\t').collect();

                assert_eq!(
                    fields.len(),
                    3,
                    "malformed golden line (regenerate the fixture): {line:?}"
                );

                let label = fields[0].to_owned();
                let bits = fields[1]
                    .parse()
                    .unwrap_or_else(|e| panic!("bits in {label:?}: {e}"));

                (label, Vector { bits, bytes: from_hex(fields[2].trim()) })
            })
            .collect()
    })
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            // A trailing nibble can appear when the used-byte count rounds up; pad it rather
            // than panicking.
            let mut digits = hex[i..(i + 2).min(hex.len())].to_owned();

            while digits.len() < 2 {
                digits.push('0');
            }

            u8::from_str_radix(&digits, 16).expect("valid hex")
        })
        .collect()
}
