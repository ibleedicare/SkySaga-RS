//! `ClientHomeIslandCustomisationComponent`: what a home island calls itself.
//!
//! The home banner (`FUN_007dacf0`) reads "<owner>'s <title>". The title is the `title` of a
//! `HomeIslandCustomisation` entity, resolved through `HomeIslandTitles`; with no such entity
//! it is "Home". Despite the `"home"` default in the data file the parameter is not a string:
//! the reader `FUN_0090e2d0` takes one presence bit and a name hash.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HomeIslandCustomisationComponent {
    /// Name hash of a `HomeIslandTitles` row, such as `Castle`.
    pub title: Option<u32>,
}

impl HomeIslandCustomisationComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "title" => writer.write_optional_u32(self.title),
            _ => return false,
        }

        true
    }
}
