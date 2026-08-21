//! Bit maths the client's serialization depends on.

/// Number of leading zero bits in `max` — the client's "how many high bits can I skip".
///
/// This is a port of `SkySaga.Game/Util.cs::NumBitsRequiredUInt32`, which is a hand-rolled
/// binary search over the top set bit. It computes exactly `u32::leading_zeros`; the tests
/// below prove the two agree, exhaustively for `u8` and over every boundary that matters
/// for `u32`. The name is kept for grep-ability against the C#.
///
/// The 18-bit right-aligned length field in `EntityAdd` and `Entity::GetSyncData` is
/// `32 - num_bits_required(0x20000)`.
#[inline]
pub const fn num_bits_required(max: u32) -> u32 {
    max.leading_zeros()
}

/// Byte-width variant, a port of `Util.cs::NumBitsRequiredByte`. Same story.
#[inline]
pub const fn num_bits_required_byte(max: u8) -> u32 {
    max.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::{num_bits_required, num_bits_required_byte};

    /// Literal transliteration of `Util.cs::NumBitsRequiredUInt32`, kept only as a test
    /// oracle so the one-line implementation can be shown to be equivalent.
    fn csharp_num_bits_required_u32(mut max: u32) -> u32 {
        let mut required = 32u32;

        if (max >> 16) as u16 > 0 {
            required = 16;
            max = u32::from((max >> 16) as u16);
        }

        if max >> 8 > 0 {
            required -= 8;
            max >>= 8;
        }

        if max >> 4 > 0 {
            required -= 4;
            max >>= 4;
        }

        if max >> 2 > 0 {
            required -= 2;
            max >>= 2;
        }

        if max & (u32::MAX - 1) != 0 {
            required - 2
        } else {
            required - max
        }
    }

    /// Literal transliteration of `Util.cs::NumBitsRequiredByte`.
    fn csharp_num_bits_required_byte(mut max: u8) -> u32 {
        let mut required = 8u32;

        if max >> 4 > 0 {
            required -= 4;
            max >>= 4;
        }

        if max >> 2 > 0 {
            required -= 2;
            max >>= 2;
        }

        if max & (u8::MAX - 1) != 0 {
            required - 2
        } else {
            required - u32::from(max)
        }
    }

    #[test]
    fn byte_version_agrees_with_the_csharp_over_every_input() {
        for max in 0..=u8::MAX {
            assert_eq!(
                num_bits_required_byte(max),
                csharp_num_bits_required_byte(max),
                "byte input {max}"
            );
        }
    }

    #[test]
    fn u32_version_agrees_with_the_csharp_at_every_boundary() {
        let mut cases: Vec<u32> = (0..=0x2_0000).collect();

        for shift in 0..32 {
            let power = 1u32 << shift;

            cases.push(power);
            cases.push(power.wrapping_sub(1));
            cases.push(power.wrapping_add(1));
            cases.push(power.wrapping_sub(2));
            cases.push(power.wrapping_add(2));
        }

        cases.push(u32::MAX);

        for max in cases {
            assert_eq!(
                num_bits_required(max),
                csharp_num_bits_required_u32(max),
                "u32 input {max:#x}"
            );
        }
    }

    /// The `EntityAdd` / `GetSyncData` length field. If this is not 18, entities are
    /// malformed and the client hangs after "Ready to Play".
    #[test]
    fn entity_length_field_is_eighteen_bits() {
        assert_eq!(32 - num_bits_required(0x2_0000), 18);
    }
}
