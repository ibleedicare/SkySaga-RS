//! The name hash the client uses for every entity, item and currency name on the wire.
//!
//! Ported from `SkySaga.Game/Util.cs::ComputeCrc32`. It is a CRC-32 with polynomial
//! `0x04C11DB7`, MSB-first and *not* reflected, initial value `0`, and no final XOR —
//! i.e. not any of the common named CRC-32 variants. The input is lowercased first.
//!
//! The C# ships a literal 256-entry table. It is exactly the standard table for that
//! polynomial, so it is generated here instead of copied.

/// Lookup table for polynomial `0x04C11DB7`, MSB-first.
const TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;

    while i < 256 {
        let mut entry = (i as u32) << 24;
        let mut bit = 0;

        while bit < 8 {
            entry = if entry & 0x8000_0000 != 0 {
                (entry << 1) ^ 0x04C1_1DB7
            } else {
                entry << 1
            };
            bit += 1;
        }

        table[i] = entry;
        i += 1;
    }

    table
}

/// Hash a name the way the client does: ASCII-lowercase, then CRC-32 as above.
///
/// ```
/// assert_eq!(skysaga_core::name_hash("Sky_Island"), 0xCBBF_A7BF);
/// ```
pub fn name_hash(name: &str) -> u32 {
    let mut crc = 0u32;

    for byte in name.as_bytes() {
        let byte = byte.to_ascii_lowercase();
        let index = (((crc >> 24) ^ u32::from(byte)) & 0xFF) as usize;

        crc = TABLE[index] ^ (crc << 8);
    }

    crc
}

#[cfg(test)]
mod tests {
    use super::name_hash;

    /// Known-good vectors taken from the C# implementation.
    #[test]
    fn matches_the_csharp_implementation() {
        let vectors: &[(&str, u32)] = &[
            ("Sky_Island", 0xCBBF_A7BF),
            ("Life_Ticket", 0xB24A_6E48),
            ("Portal_Ticket", 0x161A_6AF1),
            ("Home_Island_Adventure", 0x2619_24A2),
            ("ExplorerArmourHead", 0xD2BB_8299),
            ("ExplorerArmourTorso", 0xA833_7067),
            ("Dirt", 0x27DA_D773),
            ("Player", 0xCDE4_B742),
            ("AirShip", 0x4B51_0591),
            ("TimeOfDay", 0x0B5B_D50D),
        ];

        for &(name, expected) in vectors {
            assert_eq!(name_hash(name), expected, "hash of {name:?}");
        }
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(name_hash("Dirt"), name_hash("DIRT"));
        assert_eq!(name_hash("Dirt"), name_hash("dirt"));
    }

    #[test]
    fn empty_name_hashes_to_zero() {
        assert_eq!(name_hash(""), 0);
    }
}
