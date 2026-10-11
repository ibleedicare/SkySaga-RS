//! `ClientAdventureNameComponent`: the name a world that is not a home island goes by.
//!
//! The banner of such a world is built by `FUN_007dafa0`, which looks for an `AdventureName`
//! entity (`FUN_007daf00`) and composes the text from it (`FUN_008335b0`). With no such
//! entity it writes nothing and shows whatever the banner last said, which after a portal is
//! the home island's "<name>'s Home" (seen under gdb, 2026-10-11).
//!
//! Parameter reader `FUN_008ed270`, list reader `FUN_008ecf20`:
//!
//! | parameter | encoding |
//! |---|---|
//! | `randomseed` | 32 bits (`FUN_0073c720`) |
//! | `nametemplate` | count ranged to 5, so 3 bits, with an escape bit at 5; then per part a type ranged to 9, so 4 bits, and an optional u32 |
//!
//! The banner is built once, on entering gameplay, and nothing refreshes it afterwards: the
//! entity has to be in the join burst.

use skysaga_proto::bitstream::{ranged_bits, BitWriter};

/// The most parts a template is optimised around.
const MAX_PARTS: u32 = 5;

/// What one part of a name is drawn from. `FUN_008335b0` gives each its meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePart {
    /// A word from `AdventureNamePrefixes`, picked with the seed.
    Prefix,
    /// A word from `AdventureNameSuffixes`, picked with the seed.
    Suffix,
    /// The name part of the resource with this name hash.
    Resource(u32),
    /// The biome's own word.
    Biome,
    /// The adventure's own word.
    Adventure,
    Space,
}

impl NamePart {
    fn kind(self) -> u32 {
        match self {
            Self::Prefix => 0,
            Self::Suffix => 1,
            Self::Resource(_) => 3,
            Self::Biome => 5,
            Self::Adventure => 6,
            Self::Space => 8,
        }
    }

    fn resource(self) -> Option<u32> {
        match self {
            Self::Resource(hash) => Some(hash),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdventureNameComponent {
    pub seed: u32,
    pub template: Vec<NamePart>,
}

impl AdventureNameComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "randomseed" => writer.write_u32(self.seed),

            "nametemplate" => {
                let parts = &self.template[..self.template.len().min(MAX_PARTS as usize)];

                writer.write_bits_le(parts.len() as u32, ranged_bits(MAX_PARTS));

                // A list at its cap says so with a clear bit; a set one would announce a
                // 32-bit count.
                if parts.len() as u32 == MAX_PARTS {
                    writer.write_bit(false);
                }

                for part in parts {
                    writer.write_bits_le(part.kind(), ranged_bits(9));
                    writer.write_optional_u32(part.resource());
                }
            }

            _ => return false,
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skysaga_proto::bitstream::BitReader;

    /// Three bits written straight after a parameter: reading them back proves the parameter
    /// was exactly as long as the test walked.
    const MARK: u32 = 0b101;

    fn written(component: &AdventureNameComponent, parameter: &str) -> Vec<u8> {
        let mut writer = BitWriter::new();

        assert!(component.sync(parameter, &mut writer));

        writer.write_bits_le(MARK, 3);

        writer.into_bytes()
    }

    #[test]
    fn a_prefix_and_the_biome_are_two_bare_parts_after_the_count() {
        let bytes = written(
            &AdventureNameComponent {
                seed: 7,
                template: vec![NamePart::Prefix, NamePart::Biome],
            },
            "nametemplate",
        );

        let mut reader = BitReader::from_bytes(&bytes);

        assert_eq!(reader.read_bits_le(3).unwrap(), 2);
        assert_eq!(reader.read_bits_le(4).unwrap(), 0);
        assert!(!reader.read_bit().unwrap());
        assert_eq!(reader.read_bits_le(4).unwrap(), 5);
        assert!(!reader.read_bit().unwrap());
        assert_eq!(reader.read_bits_le(3).unwrap(), MARK);
    }

    #[test]
    fn a_resource_part_carries_its_hash() {
        let bytes = written(
            &AdventureNameComponent {
                seed: 0,
                template: vec![NamePart::Resource(0xdead_beef)],
            },
            "nametemplate",
        );

        let mut reader = BitReader::from_bytes(&bytes);

        assert_eq!(reader.read_bits_le(3).unwrap(), 1);
        assert_eq!(reader.read_bits_le(4).unwrap(), 3);
        assert!(reader.read_bit().unwrap());
        assert_eq!(reader.read_u32().unwrap(), 0xdead_beef);
        assert_eq!(reader.read_bits_le(3).unwrap(), MARK);
    }

    #[test]
    fn five_parts_are_followed_by_a_clear_escape_bit() {
        let bytes = written(
            &AdventureNameComponent {
                seed: 0,
                template: vec![NamePart::Space; 7],
            },
            "nametemplate",
        );

        let mut reader = BitReader::from_bytes(&bytes);

        assert_eq!(reader.read_bits_le(3).unwrap(), 5);
        assert!(!reader.read_bit().unwrap());

        for _ in 0..5 {
            assert_eq!(reader.read_bits_le(4).unwrap(), 8);
            assert!(!reader.read_bit().unwrap());
        }

        assert_eq!(reader.read_bits_le(3).unwrap(), MARK);
    }

    #[test]
    fn the_seed_is_thirty_two_bits() {
        let bytes = written(
            &AdventureNameComponent {
                seed: 0x1234_5678,
                template: Vec::new(),
            },
            "randomseed",
        );

        let mut reader = BitReader::from_bytes(&bytes);

        assert_eq!(reader.read_u32().unwrap(), 0x1234_5678);
        assert_eq!(reader.read_bits_le(3).unwrap(), MARK);
    }
}
