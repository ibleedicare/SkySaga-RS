//! Fixed-width NUL-padded ASCII fields.
//!
//! The Smilegate auth protocol lays its strings out the way C#'s
//! `[MarshalAs(UnmanagedType.ByValTStr, SizeConst = N)]` does: a fixed `N`-byte field
//! holding the string followed by a NUL, with the remainder zero-filled. The string is read
//! back as the bytes up to the first NUL.

/// Read a fixed-width field: the bytes before the first NUL, lossily decoded.
///
/// A field with no NUL at all is taken whole — the C# marshaller reserves the last byte for
/// the terminator, but the client is not guaranteed to be that disciplined and truncating
/// silently is worse than accepting it.
pub fn read(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());

    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// Write a fixed-width field: `value`, then a NUL, then zero-fill.
///
/// Truncates rather than panicking when `value` does not fit, always leaving room for the
/// terminator so [`read`] round-trips whatever survives. Truncation happens on a UTF-8
/// character boundary, so the field never contains a partial code point.
pub fn write(field: &mut [u8], value: &str) {
    field.fill(0);

    if field.is_empty() {
        return;
    }

    let capacity = field.len() - 1;
    let mut end = value.len().min(capacity);

    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }

    field[..end].copy_from_slice(&value.as_bytes()[..end]);
}

#[cfg(test)]
mod tests {
    use super::{read, write};

    #[test]
    fn round_trips_an_ordinary_name() {
        let mut field = [0xAAu8; 50];

        write(&mut field, "Alice");

        assert_eq!(&field[..6], b"Alice\0");
        assert!(field[6..].iter().all(|&b| b == 0), "tail must be zero-filled");
        assert_eq!(read(&field), "Alice");
    }

    #[test]
    fn round_trips_an_empty_string() {
        let mut field = [0xAAu8; 8];

        write(&mut field, "");

        assert_eq!(field, [0u8; 8]);
        assert_eq!(read(&field), "");
    }

    /// A value of exactly `len - 1` fits with its terminator; `len` does not.
    #[test]
    fn reserves_a_byte_for_the_terminator() {
        let mut field = [0u8; 4];

        write(&mut field, "abc");
        assert_eq!(field, *b"abc\0");
        assert_eq!(read(&field), "abc");

        write(&mut field, "abcd");
        assert_eq!(field, *b"abc\0");
        assert_eq!(read(&field), "abc");
    }

    #[test]
    fn truncates_instead_of_panicking() {
        let mut field = [0u8; 4];

        write(&mut field, "a very long account name");

        assert_eq!(read(&field), "a v");
    }

    #[test]
    fn never_truncates_mid_character() {
        let mut field = [0u8; 4];

        // 'é' is two bytes, so only one of them would fit in the three usable bytes.
        write(&mut field, "aaé");

        assert_eq!(read(&field), "aa");
    }

    #[test]
    fn reads_a_field_with_no_terminator() {
        assert_eq!(read(b"abcd"), "abcd");
    }

    #[test]
    fn reads_stop_at_the_first_nul() {
        assert_eq!(read(b"ab\0cd\0"), "ab");
    }

    #[test]
    fn zero_length_field_is_a_no_op() {
        let mut field: [u8; 0] = [];

        write(&mut field, "anything");

        assert_eq!(read(&field), "");
    }
}
