//! The todo list: the quest log the client calls its challenge tracker.
//!
//! # One record, two directions
//!
//! The server sends the whole list as `TodoListComponent.tasklist` (Player sync index 82); the
//! client asks for changes with four small packets. The row in the list and the body of
//! [`TodoListTaskAdd`] are **the same 48-byte record**, written by the same primitives in the
//! same order — `FUN_008a9e30` (the list) and `FUN_0084b570` (the packet) agree field for
//! field. [`TodoTask`] is therefore shared by both.
//!
//! ```text
//! server                                          client
//!   TodoListComponent.tasklist ──EntitySync(82)──▶ renders the tracker
//!                              ◀──TodoListTaskAdd── player adds an objective
//!                              ◀──Erase/Remove/ReAdd(taskId)──
//! ```
//!
//! # The three verbs are not synonyms
//!
//! *Erase* deletes a task for good, *Remove* takes it off the visible list, and *ReAdd* puts a
//! removed one back. That is read from the names; the callers were not traced, so the server
//! treats Remove and ReAdd as a visibility flip and only Erase as a deletion.
//!
//! # Widths
//!
//! Every one is read from the client's own writers rather than inferred:
//!
//! | field | bits | from |
//! |---|---|---|
//! | `task_id` | 8 | `FUN_0084a5d0`, `0x20 - CLZ(0xff)` |
//! | objective count | 1 + 7, or 1 + 17 over 64 | `FUN_00794dc0` |
//! | objective C's value | 32 flat | `FUN_00778da0` |
//! | `timed_challenge_type` | 2 | `FUN_008d11d0`, `0x20 - CLZ(3)` |
//! | list count | 6 (+ escape, max 32) | `FUN_008a96e0` |
//!
//! The one capture in `logs/game.log:31599` is 9 bytes, so 56 bits of body after the two-byte
//! extended id. One objective present with a resource and a small count, the other two absent:
//! `8 + 1 + (1+1+32+1+7) + 1 + 1 + 2 + 1 = 56`. Exact.

use crate::bitstream::{ranged_bits, BitError, BitReader, BitWriter};

/// The most tasks the list holds, which sets the count width.
pub const TASK_LIST_DEFAULT: u32 = 32;

/// A count at or below this is 7 bits; above it, 17. `FUN_00794dc0`.
const SMALL_COUNT_MAX: u32 = 64;

/// The width of the wide form, `0x20 - CLZ(0x10000)`.
const WIDE_COUNT_BITS: u32 = ranged_bits(0x10000);

/// `timedJobChallengeType`, two bits.
///
/// `0` none, `1` Daily, `2` Weekly. Only the **width** is proven (max 3, from `FUN_008d11d0`);
/// the mapping is inferred from the `Jobs` table's `Daily` and `Weekly` rows and geodata's
/// 260/14 split of `UnlockedBy.TimedChallengeType`.
const MAX_TIMED_TYPE: u32 = 3;

/// An objective counted in items: "collect N of X".
///
/// Objectives A and B of a task. `resource` is optional independently of the objective's own
/// presence, so "present, but no particular resource" is expressible and is written as a clear
/// optional bit followed by the count anyway (`FUN_0084b140`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemObjective {
    /// Name hash of the resource wanted, or `None`.
    pub resource: Option<u32>,

    pub count: u32,
}

/// An objective counted in something other than items, carrying a flat 32-bit value.
///
/// Objective C. Structurally the same as [`ItemObjective`] but for the value, which is a full
/// word rather than the 7/17 form — a difference the client is explicit about
/// (`FUN_0084b1c0` calls `FUN_00778da0`, not `FUN_00794dc0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ValueObjective {
    pub resource: Option<u32>,

    pub value: u32,
}

/// One row of the quest log.
///
/// 48 bytes in the client. The bare `flag` bit has never been observed set and its meaning is
/// unknown; it is carried through rather than guessed at, so a task the client sends back
/// round-trips unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TodoTask {
    /// Server-allocated, 8 bits, and the only thing Erase/Remove/ReAdd carry.
    pub task_id: u8,

    /// Meaning unknown; preserved verbatim.
    pub flag: bool,

    pub objective_a: Option<ItemObjective>,
    pub objective_b: Option<ItemObjective>,
    pub objective_c: Option<ValueObjective>,

    pub timed_challenge_type: u32,
}

/// `min(count, max)`, then at the cap a bit saying whether a real 32-bit count follows.
///
/// At the cap *exactly* that bit is clear. See `skysaga_world::components::write_count` — the
/// same rule, and getting its polarity backwards shifts everything downstream.
fn write_list_count(writer: &mut BitWriter, count: u32, max: u32) {
    writer.write_bits_le(count.min(max), ranged_bits(max));

    if count < max {
        return;
    }

    if count == max {
        writer.write_bit(false);
    } else {
        writer.write_bit(true);
        writer.write_u32(count);
    }
}

fn read_list_count(reader: &mut BitReader, max: u32) -> Result<u32, BitError> {
    let count = reader.read_bits_le(ranged_bits(max))?;

    if count < max {
        return Ok(count);
    }

    if reader.read_bit()? {
        reader.read_u32()
    } else {
        Ok(max)
    }
}

/// An item count: one bit choosing the width, then 7 or 17 bits. `FUN_00794dc0`.
fn write_item_count(writer: &mut BitWriter, count: u32) {
    if count <= SMALL_COUNT_MAX {
        writer.write_bit(false);
        writer.write_bits_le(count, ranged_bits(SMALL_COUNT_MAX));
    } else {
        writer.write_bit(true);
        writer.write_bits_le(count, WIDE_COUNT_BITS);
    }
}

fn read_item_count(reader: &mut BitReader) -> Result<u32, BitError> {
    if reader.read_bit()? {
        reader.read_bits_le(WIDE_COUNT_BITS)
    } else {
        reader.read_bits_le(ranged_bits(SMALL_COUNT_MAX))
    }
}

impl ItemObjective {
    fn encode_optional(this: Option<Self>, writer: &mut BitWriter) {
        let Some(objective) = this else {
            writer.write_bit(false);
            return;
        };

        writer.write_bit(true);
        writer.write_optional_u32(objective.resource);
        write_item_count(writer, objective.count);
    }

    fn decode_optional(reader: &mut BitReader) -> Result<Option<Self>, BitError> {
        if !reader.read_bit()? {
            return Ok(None);
        }

        Ok(Some(Self {
            resource: reader.read_optional_u32()?,
            count: read_item_count(reader)?,
        }))
    }
}

impl ValueObjective {
    fn encode_optional(this: Option<Self>, writer: &mut BitWriter) {
        let Some(objective) = this else {
            writer.write_bit(false);
            return;
        };

        writer.write_bit(true);
        writer.write_optional_u32(objective.resource);
        writer.write_u32(objective.value);
    }

    fn decode_optional(reader: &mut BitReader) -> Result<Option<Self>, BitError> {
        if !reader.read_bit()? {
            return Ok(None);
        }

        Ok(Some(Self {
            resource: reader.read_optional_u32()?,
            value: reader.read_u32()?,
        }))
    }
}

impl TodoTask {
    /// The record, without any packet id. Shared with the component's list.
    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_bits_le(u32::from(self.task_id), 8);
        writer.write_bit(self.flag);

        ItemObjective::encode_optional(self.objective_a, writer);
        ItemObjective::encode_optional(self.objective_b, writer);
        ValueObjective::encode_optional(self.objective_c, writer);

        writer.write_bits_le(
            self.timed_challenge_type.min(MAX_TIMED_TYPE),
            ranged_bits(MAX_TIMED_TYPE),
        );
    }

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            task_id: reader.read_bits_le(8)? as u8,
            flag: reader.read_bit()?,
            objective_a: ItemObjective::decode_optional(reader)?,
            objective_b: ItemObjective::decode_optional(reader)?,
            objective_c: ValueObjective::decode_optional(reader)?,
            timed_challenge_type: reader.read_bits_le(ranged_bits(MAX_TIMED_TYPE))?,
        })
    }

    /// The whole `tasklist` parameter, count and all.
    pub fn encode_list(tasks: &[Self], writer: &mut BitWriter) {
        write_list_count(writer, tasks.len() as u32, TASK_LIST_DEFAULT);

        for task in tasks {
            task.encode(writer);
        }
    }

    pub fn decode_list(reader: &mut BitReader) -> Result<Vec<Self>, BitError> {
        let count = read_list_count(reader, TASK_LIST_DEFAULT)?;

        (0..count).map(|_| Self::decode(reader)).collect()
    }
}

/// `TodoListTaskAdd` (146, `FF 19`) — the player put an objective on the list.
///
/// `FUN_0084b570`. The body is a [`TodoTask`] plus one bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TodoListTaskAdd {
    pub task: TodoTask,

    /// Set when the player added it by hand rather than a challenge auto-adding it.
    ///
    /// Two Tutorial challenges (`Add_own_quest`, `Add_own_quest_when_empty`) exist purely to
    /// teach this, so the manual path is itself an objective.
    pub manually_added: bool,
}

impl TodoListTaskAdd {
    pub const ID: u16 = 146;

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            task: TodoTask::decode(reader)?,
            manually_added: reader.read_bit()?,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        self.task.encode(writer);
        writer.write_bit(self.manually_added);
    }
}

/// The three packets that carry nothing but a task id.
///
/// `FUN_0084a9f0` (Erase, 145), `FUN_0084ac30` (Remove, 147) and `FUN_0084ae70` (ReAdd, 148)
/// are byte-for-byte identical; only the ordinal distinguishes them. The JSON key is spelled
/// `TaskID`, which is what retro-names field 1 of [`TodoListTaskAdd`] too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TodoListTaskRef {
    pub task_id: u8,
}

impl TodoListTaskRef {
    pub const ERASE: u16 = 145;
    pub const REMOVE: u16 = 147;
    pub const READD: u16 = 148;

    pub fn decode(reader: &mut BitReader) -> Result<Self, BitError> {
        Ok(Self {
            task_id: reader.read_bits_le(8)? as u8,
        })
    }

    pub fn encode(&self, writer: &mut BitWriter) {
        writer.write_bits_le(u32::from(self.task_id), 8);
    }
}
