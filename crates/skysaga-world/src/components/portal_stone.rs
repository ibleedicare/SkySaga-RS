//! `PortalStoneComponent`: the one parameter a portal stone entity adds to an item.
//!
//! `targetseed` is 32 bits (name table `FUN_008e3b40`). Only sent here for the stones that
//! have a lifetime, which have to be a `DurablePortalStone` to carry one; what the seed
//! selects has not been reversed, and zero is what the data file defaults to.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PortalStoneComponent {
    pub target_seed: u32,
}

impl PortalStoneComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            "targetseed" => writer.write_u32(self.target_seed),
            _ => return false,
        }

        true
    }
}
