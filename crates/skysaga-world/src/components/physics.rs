//! `ClientCharacterPhysicsComponent` / `ClientPhysicsComponent` — how an entity collides.
//!
//! Two booleans. The C# has a three-level hierarchy here (`PhysicsComponent` ->
//! `ClientPhysicsComponent` -> `ClientCharacterPhysicsComponent`) where the subclasses only
//! call `base`; flattened, since the distinction is which *name* an entity binds to.

use skysaga_proto::bitstream::BitWriter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicsComponent {
    pub fine_grain_collision_only: bool,
    pub is_moveable: bool,
}

impl Default for PhysicsComponent {
    fn default() -> Self {
        Self {
            fine_grain_collision_only: false,
            // The C# defaults this to true.
            is_moveable: true,
        }
    }
}

impl PhysicsComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        let value = match parameter.to_ascii_lowercase().as_str() {
            "finegraincollisiononly" => self.fine_grain_collision_only,
            "ismoveable" => self.is_moveable,

            _ => return false,
        };

        writer.write_bit(value);

        true
    }
}
