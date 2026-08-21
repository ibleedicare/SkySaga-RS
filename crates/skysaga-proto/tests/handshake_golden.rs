//! Loads `tests/fixtures/handshake.tsv` — packets captured off the wire from the C# server.
//!
//! Format: `label<TAB>byte count<TAB>hex`, `#` for comments.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

pub struct Capture {
    pub bytes: Vec<u8>,
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn capture(label: &str) -> &'static Capture {
    captures()
        .get(label)
        .unwrap_or_else(|| panic!("no capture named {label:?}"))
}

/// How many packets of each wire id the C# server sent.
pub fn counts_by_wire_id() -> HashMap<u16, usize> {
    let mut counts = HashMap::new();

    for label in captures().keys() {
        // server_<wire id>_<n>
        if let Some(id) = label.split('_').nth(1).and_then(|id| id.parse().ok()) {
            *counts.entry(id).or_default() += 1;
        }
    }

    counts
}

pub fn captures() -> &'static HashMap<String, Capture> {
    static CAPTURES: OnceLock<HashMap<String, Capture>> = OnceLock::new();

    CAPTURES.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/handshake.tsv");
        let text = std::fs::read_to_string(path).expect("handshake capture is present");

        text.lines()
            .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();

                assert_eq!(fields.len(), 3, "malformed capture line: {line:?}");

                let label = fields[0].to_owned();
                let declared: usize = fields[1].parse().expect("byte count");
                let bytes = from_hex(fields[2].trim());

                assert_eq!(bytes.len(), declared, "{label}: hex does not match its length");

                (label, Capture { bytes })
            })
            .collect()
    })
}

fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// Every capture label for a given wire id, in capture order.
pub fn labels_for_wire_id(id: u16) -> Vec<String> {
    let prefix = format!("server_{id}_");

    let mut labels: Vec<String> = captures()
        .keys()
        .filter(|label| label.starts_with(&prefix))
        .cloned()
        .collect();

    // server_234_2 must sort after server_234_10's *numeric* predecessor, not lexically.
    labels.sort_by_key(|label| {
        label
            .rsplit('_')
            .next()
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0)
    });

    labels
}
