//! An entity instance: an id, a definition, and the components holding its state.
//!
//! Serialising one is the whole point. Every sync index is visited in order, its component
//! asked to write that parameter, and the flag bit set only if the component actually wrote
//! something — which is how a declined parameter (`TransformComponent::yawdegrees`, an empty
//! voxel list) removes itself from the packet.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::{Bits, EntityAdd, SyncData};

use crate::{Component, EntityDefinition};

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub id: u32,
    pub components: Vec<Component>,
}

impl Entity {
    pub fn new(id: u32, components: Vec<Component>) -> Self {
        Self { id, components }
    }

    /// The component that owns `name`, if this entity has it.
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components
            .iter()
            .find(|component| component.name().eq_ignore_ascii_case(name))
    }

    /// Build this entity's sync body against its definition.
    ///
    /// `new_entity` mirrors the C#'s flag of the same name: on a fresh entity every parameter
    /// is offered, whereas an update offers only the ones that changed. Only the fresh case
    /// is implemented — dirty tracking belongs with the entity store, not here.
    pub fn sync_data(&self, definition: &EntityDefinition) -> SyncData {
        let mut present = vec![false; definition.synced_parameter_count()];
        let mut parameters = BitWriter::new();

        for index in 0..definition.synced_parameter_count() {
            let Some((component_name, parameter)) = definition.parameter_at(index) else {
                continue;
            };

            let Some(component) = self.component(component_name) else {
                // No such component on this entity: the parameter is declared but nothing
                // owns it, so it is simply absent. The C# reaches the same outcome by
                // failing to construct the class at all.
                continue;
            };

            if component.sync(parameter, &mut parameters) {
                present[index] = true;
            }
        }

        SyncData {
            present,
            parameters: Bits::from_writer(&parameters),
        }
    }

    /// The `EntityAdd` announcing this entity to a client.
    pub fn to_entity_add(&self, definition: &EntityDefinition) -> EntityAdd {
        let mut sync = BitWriter::new();

        self.sync_data(definition).encode(&mut sync);

        EntityAdd {
            name_hash: Some(definition.name_hash()),
            id: self.id,
            parent_id: None,
            sync_data: Bits::from_writer(&sync),
        }
    }
}
