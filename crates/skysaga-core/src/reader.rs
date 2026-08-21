//! A non-panicking cursor over a byte slice.
//!
//! Every read is bounds-checked and returns a `Result`, because the input is a network
//! packet from an untrusted peer. A slice index that panics in a packet handler takes down
//! every connected player.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReaderError {
    #[error("buffer too short: wanted {wanted} bytes at offset {offset}, only {available} left")]
    TooShort {
        offset: usize,
        wanted: usize,
        available: usize,
    },
}

/// Little-endian cursor over `&[u8]`.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Bytes consumed so far.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Take the next `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], ReaderError> {
        if self.remaining() < n {
            return Err(ReaderError::TooShort {
                offset: self.offset,
                wanted: n,
                available: self.remaining(),
            });
        }

        let taken = &self.data[self.offset..self.offset + n];

        self.offset += n;

        Ok(taken)
    }

    /// Take the next `N` bytes as an array, for fixed-width fields.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], ReaderError> {
        let bytes = self.bytes(N)?;
        let mut out = [0u8; N];

        out.copy_from_slice(bytes);

        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, ReaderError> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16_le(&mut self) -> Result<u16, ReaderError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32_le(&mut self) -> Result<u32, ReaderError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn i32_le(&mut self) -> Result<i32, ReaderError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    /// Read a fixed-width NUL-padded field of `N` bytes. See [`crate::fixed_str`].
    pub fn fixed_str<const N: usize>(&mut self) -> Result<String, ReaderError> {
        Ok(crate::fixed_str::read(self.bytes(N)?))
    }
}

#[cfg(test)]
mod tests {
    use super::{Reader, ReaderError};

    #[test]
    fn reads_little_endian_scalars_in_order() {
        let data = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let mut reader = Reader::new(&data);

        assert_eq!(reader.u8().unwrap(), 0x01);
        assert_eq!(reader.u16_le().unwrap(), 0x0302);
        assert_eq!(reader.u32_le().unwrap(), 0x0706_0504);
        assert!(reader.is_empty());
    }

    #[test]
    fn reads_negative_i32() {
        let data = (-2i32).to_le_bytes();

        assert_eq!(Reader::new(&data).i32_le().unwrap(), -2);
    }

    #[test]
    fn tracks_offset_and_remaining() {
        let data = [0u8; 8];
        let mut reader = Reader::new(&data);

        assert_eq!((reader.offset(), reader.remaining()), (0, 8));

        reader.bytes(3).unwrap();

        assert_eq!((reader.offset(), reader.remaining()), (3, 5));
    }

    #[test]
    fn refuses_to_read_past_the_end() {
        let data = [1u8, 2, 3];
        let mut reader = Reader::new(&data);

        assert_eq!(
            reader.u32_le(),
            Err(ReaderError::TooShort {
                offset: 0,
                wanted: 4,
                available: 3,
            })
        );
    }

    /// A failed read must not advance the cursor, so a caller can recover.
    #[test]
    fn a_failed_read_does_not_consume() {
        let data = [1u8, 2, 3];
        let mut reader = Reader::new(&data);

        assert!(reader.u32_le().is_err());
        assert_eq!(reader.offset(), 0);
        assert_eq!(reader.u16_le().unwrap(), 0x0201);
    }

    #[test]
    fn reads_fixed_width_strings() {
        let mut data = [0u8; 12];

        data[..5].copy_from_slice(b"Alice");
        data[8..11].copy_from_slice(b"Bob");

        let mut reader = Reader::new(&data);

        assert_eq!(reader.fixed_str::<8>().unwrap(), "Alice");
        assert_eq!(reader.fixed_str::<4>().unwrap(), "Bob");
    }

    #[test]
    fn zero_length_reads_always_succeed() {
        let mut reader = Reader::new(&[]);

        assert_eq!(reader.bytes(0).unwrap(), &[] as &[u8]);
    }
}
