//! `ClientTimeOfDayComponent` — the world clock.
//!
//! Six parameters, 123 bits in total, which is exactly the payload size of the captured
//! `TimeOfDay` entity. That the widths add up to the observed total *before* any byte
//! comparison is the first check that they are right.
//!
//! | index | parameter | bits | |
//! |---|---|---:|---|
//! | 0 | `daynightcycleduration` | 11 | ranged, max 1920 |
//! | 1 | `fixedtimeofday` | 1 | bool |
//! | 2 | `realworldstarttime` | 64 | `WriteUInt64`, little-endian |
//! | 3 | `starttimeofday` | 17 | ranged, max 0x10000 |
//! | 4 | `timeofdayoffset` | 17 | ranged, max 0x10000 |
//! | 5 | `timestretch` | 13 | ranged, max 8128 |
//!
//! Parameters are written in sync-index order, which is alphabetical here because that is how
//! the indices happen to fall — not a rule.

use skysaga_proto::bitstream::{BitError, BitReader, BitWriter};

use super::ranged_bits;

/// Declared maxima, which set the field widths. Part of the protocol, not tuning knobs.
const MAX_TIME_OF_DAY: u32 = 0x1_0000;
const MAX_CYCLE_DURATION: u32 = 1920;
const MAX_TIME_STRETCH: u32 = 8128;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeOfDayComponent {
    pub day_night_cycle_duration: u32,
    pub fixed_time_of_day: bool,
    /// Milliseconds since some epoch the client picks; the server only echoes it.
    pub real_world_start_time: u64,
    pub start_time_of_day: u32,
    pub time_of_day_offset: u32,
    pub time_stretch: u32,
}

impl TimeOfDayComponent {
    /// Total width of all six parameters, when every one is synced.
    pub const SYNCED_BITS: usize = 11 + 1 + 64 + 17 + 17 + 13;

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "daynightcycleduration" => {
                writer.write_bits_le(self.day_night_cycle_duration, ranged_bits(MAX_CYCLE_DURATION))
            }
            "fixedtimeofday" => writer.write_bit(self.fixed_time_of_day),
            "realworldstarttime" => writer.write_u64_le(self.real_world_start_time),
            "starttimeofday" => {
                writer.write_bits_le(self.start_time_of_day, ranged_bits(MAX_TIME_OF_DAY))
            }
            "timeofdayoffset" => {
                writer.write_bits_le(self.time_of_day_offset, ranged_bits(MAX_TIME_OF_DAY))
            }
            "timestretch" => writer.write_bits_le(self.time_stretch, ranged_bits(MAX_TIME_STRETCH)),

            _ => return false,
        }

        true
    }

    /// Read all six parameters back, in sync-index order.
    ///
    /// Only valid when every parameter is present, which is what the captured entity does.
    pub fn decode_all(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            day_night_cycle_duration: reader.read_bits_le(ranged_bits(MAX_CYCLE_DURATION))?,
            fixed_time_of_day: reader.read_bit()?,
            real_world_start_time: reader.read_u64_le()?,
            start_time_of_day: reader.read_bits_le(ranged_bits(MAX_TIME_OF_DAY))?,
            time_of_day_offset: reader.read_bits_le(ranged_bits(MAX_TIME_OF_DAY))?,
            time_stretch: reader.read_bits_le(ranged_bits(MAX_TIME_STRETCH))?,
        })
    }

    /// Write all six, in sync-index order.
    pub fn encode_all(&self, writer: &mut BitWriter) {
        for parameter in [
            "daynightcycleduration",
            "fixedtimeofday",
            "realworldstarttime",
            "starttimeofday",
            "timeofdayoffset",
            "timestretch",
        ] {
            self.sync(parameter, writer);
        }
    }
}
