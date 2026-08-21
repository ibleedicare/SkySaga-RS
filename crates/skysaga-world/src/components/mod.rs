//! Components: the things that hold an entity's replicated state.
//!
//! # How a component reaches the wire
//!
//! An entity declares parameters with sync indices; each index names a
//! `(component, parameter)` pair (see [`crate::definitions`]). When an entity is serialised,
//! every index is visited in order and its component asked to write that parameter. Whether
//! it wrote anything is what sets the flag bit, so a component that declines a parameter
//! silently removes it from the packet.
//!
//! # Adding a component
//!
//! One struct, one variant on [`Component`], one arm in [`Component::sync`]. The `match` is
//! exhaustive, so a missing arm is a compile error rather than a parameter that quietly stops
//! replicating. No reflection, no registry keyed by string — the C# uses
//! `Activator.CreateInstance` over class names, which is why a missing class there is a
//! silent no-op (that is the reason `clientcharactercustomisationcomponent` never attaches).
//!
//! # Bit widths
//!
//! Almost every field is a *ranged* integer: `32 - num_bits_required(max)` bits, written with
//! the little-endian `write_bits_le` idiom. The declared maximum is part of the protocol, so
//! it is named as a constant next to each field rather than inlined.

pub mod time_of_day;

pub use time_of_day::TimeOfDayComponent;

use skysaga_proto::bitstream::BitWriter;

/// Width of a ranged field whose declared maximum is `max`.
///
/// The client computes `32 - NumBitsRequired(max)`, which is `32 - leading_zeros(max)`.
pub(crate) const fn ranged_bits(max: u32) -> u32 {
    32 - max.leading_zeros()
}

/// Every component the server implements.
#[derive(Debug, Clone, PartialEq)]
pub enum Component {
    TimeOfDay(TimeOfDayComponent),
}

impl Component {
    /// The component's name as it appears in `Entities.json` — lower-case, no separators.
    pub fn name(&self) -> &'static str {
        match self {
            Self::TimeOfDay(_) => "clienttimeofdaycomponent",
        }
    }

    /// Write `parameter` to `writer`, reporting whether it was written.
    ///
    /// `false` means "not mine", and leaves the writer untouched — the caller uses that to
    /// decide whether to set the parameter's flag bit.
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match self {
            Self::TimeOfDay(component) => component.sync(parameter, writer),
        }
    }
}
