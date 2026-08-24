//! `JobRankComponent`, what the player has ranked up, which is what unlocks recipes.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `joblist` | 7 + 68 each | count-optimised, default 64 |
//!
//! The component declares seven parameters; only the first is implemented. The other six are
//! challenge bookkeeping that nothing reads yet, and they are **declined** rather than written
//! empty: a parameter that reports success gets its flag set, and a set flag over no payload
//! shifts every parameter after it.
//!
//! # Why crafting needs this
//!
//! Every recipe carries a `RequiredJob` and a `RequiredJobRank`, and the client checks them
//! before it will craft. Knowing a recipe is not enough. With no job list the answer is
//! "Advance in the tutorial to unlock this item" for everything, which points at the recipe
//! book rather than at the component actually missing.

use skysaga_proto::bitstream::BitWriter;

use super::{ranged_bits, write_count};

/// `joblist` is a `[0, 64]` list.
const JOB_LIST_DEFAULT: usize = 64;

/// A rank is a byte clamped to 200, so `8 - CLZ8(200)` is eight bits.
const MAX_RANK: u8 = 200;

/// Experience is capped at ten thousand, which is fourteen bits.
const MAX_EXPERIENCE: u32 = 10_000;

/// One job the player has a rank in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobRank {
    /// `name_hash` of the job's name from `geodata.json > Jobs`.
    pub name: u32,

    pub rank: u8,

    /// Two experience values, fourteen bits each.
    ///
    /// Almost certainly the current total and the amount the next rank needs, but which is
    /// which was never established. Both go out as zero, which says "no progress" under either
    /// reading; a rank is what gates a recipe and the rank is exact.
    pub experience: u32,
    pub experience_to_next: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobRankComponent {
    pub jobs: Vec<JobRank>,
}

impl JobRankComponent {
    const RANK_BITS: u32 = 8 - MAX_RANK.leading_zeros();
    const EXPERIENCE_BITS: u32 = ranged_bits(MAX_EXPERIENCE);

    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "joblist" => {
                write_count(writer, self.jobs.len(), JOB_LIST_DEFAULT);

                for job in &self.jobs {
                    writer.write_optional_u32(Some(job.name));
                    writer.write_bits_le(u32::from(job.rank.min(MAX_RANK)), Self::RANK_BITS);

                    for experience in [job.experience, job.experience_to_next] {
                        writer.write_bits_le(experience.min(MAX_EXPERIENCE), Self::EXPERIENCE_BITS);
                    }
                }
            }

            _ => return false,
        }

        true
    }
}
