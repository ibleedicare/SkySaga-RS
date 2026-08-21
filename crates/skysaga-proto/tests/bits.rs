//! A bit-string builder for the 36731 layout tests.
//!
//! Deliberately naive, and deliberately sharing no code with `BitWriter`: these tests describe
//! the format the writer is checked against, and a builder that reused the writer's primitives
//! would only prove the writer agrees with itself.
//!
//! There is no capture to check the 2017 packets against and there cannot be one — they are
//! server->client only — so this stands in for the golden fixtures the 10414 tests use.

#![allow(dead_code)]

use skysaga_proto::bitstream::BitWriter;

/// Bits as `'0'`/`'1'` characters, in the order they go on the wire.
#[derive(Default)]
pub struct Expected(pub String);

impl Expected {
    pub fn bit(&mut self, set: bool) -> &mut Self {
        self.0.push(if set { '1' } else { '0' });
        self
    }

    /// One byte, most-significant bit first — how RakNet fills a byte.
    pub fn byte(&mut self, value: u8) -> &mut Self {
        for index in (0..8).rev() {
            self.bit(value & (1 << index) != 0);
        }
        self
    }

    pub fn bytes(&mut self, values: &[u8]) -> &mut Self {
        for &value in values {
            self.byte(value);
        }
        self
    }

    /// A ranged integer, as `WriteBits(BitConverter.GetBytes(v), width, rightAligned: true)`.
    ///
    /// The bytes are consumed **little-endian**, each whole byte contributing all eight bits
    /// most-significant-first, and a trailing partial byte contributing its *low* bits, also
    /// most-significant-first. RakNet gets there by left-shifting that byte first, which is
    /// where "right aligned" comes from.
    ///
    /// Below eight bits this is indistinguishable from "the low bits of the value, MSB first",
    /// which is why every field in this build's `MapSpec` and `ServerInfo` looks like the
    /// simple form. At fifteen bits, as in `SetConnectionTimeout`, it is not: byte order shows.
    pub fn ranged(&mut self, value: u32, width: u32) -> &mut Self {
        let bytes = value.to_le_bytes();
        let mut remaining = width;
        let mut index = 0;

        while remaining > 0 {
            let byte = bytes.get(index).copied().unwrap_or(0);

            if remaining >= 8 {
                self.byte(byte);
                remaining -= 8;
            } else {
                self.byte(byte << (8 - remaining));
                self.0.truncate(self.0.len() - (8 - remaining) as usize);
                remaining = 0;
            }

            index += 1;
        }

        self
    }

    /// `hasData` bit, `largeLength` bit, an 8-bit length, then the bytes. Empty is one `0` bit.
    pub fn string(&mut self, value: &str) -> &mut Self {
        if value.is_empty() {
            return self.bit(false);
        }

        self.bit(true).bit(false).byte(value.len() as u8);
        self.bytes(value.as_bytes())
    }

    /// A presence bit, then the 16 bytes if present.
    pub fn optional_uuid(&mut self, value: Option<[u8; 16]>) -> &mut Self {
        self.bit(value.is_some());

        match value {
            Some(bytes) => self.bytes(&bytes),
            None => self,
        }
    }

    /// A big-endian 16-bit field — RakNet's own `Write<T>`, not the ranged idiom.
    pub fn u16_be(&mut self, value: u16) -> &mut Self {
        self.bytes(&value.to_be_bytes())
    }

    pub fn u32_be(&mut self, value: u32) -> &mut Self {
        self.bytes(&value.to_be_bytes())
    }
}

/// The writer's output as `'0'`/`'1'`, truncated to the bits actually written.
///
/// The final partial byte is zero-padded in the buffer, and comparing padding would assert
/// something the protocol does not care about.
pub fn actual(writer: &BitWriter) -> String {
    let mut bits = String::new();

    for index in 0..writer.bits_used() {
        let byte = writer.as_bytes()[index / 8];

        bits.push(if byte & (0x80 >> (index % 8)) != 0 {
            '1'
        } else {
            '0'
        });
    }

    bits
}
