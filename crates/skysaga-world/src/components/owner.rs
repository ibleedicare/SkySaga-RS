//! `ClientOwnerComponent` — whose entity this is.
//!
//! One string: the owning character's uuid. The client compares it against `ServerInfo`'s
//! `owner_guid` to decide whether the world is yours, which gates the home-island placement
//! rules ("this item can only be placed on your home island").

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnerComponent {
    pub owner: String,
}

impl OwnerComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "owner" => writer.write_string(&self.owner),
            _ => return false,
        }

        true
    }
}
