//! `TeacherComponent`: what an item teaches when it is used.
//!
//! A title deed's tooltip is "'%S' Title Deed" and "rename your home island to '%S'": the
//! name is not the item's, it is looked up from the `teachitemcrc` of the stack, which only a
//! `TeacherInventoryItem` carries. A deed sent as a plain item shows the bare `%S` (seen
//! 2026-10-11).
//!
//! One parameter, read by `FUN_0090e2d0` (sync vtable `00d040e0`): a presence bit and a name
//! hash, the same reader as a home island's `title`.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeacherComponent {
    /// Name hash of what is taught: a `HomeIslandTitles` row for a title deed.
    pub teaches: Option<u32>,
}

impl TeacherComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "teachitemcrc" => writer.write_optional_u32(self.teaches),
            _ => return false,
        }

        true
    }
}
