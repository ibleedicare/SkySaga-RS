//! What the client is actually told about the seeded chest.
//!
//! `cargo run -p skysaga-game --example describe-chest`
//!
//! Prints the chest's `EntityAdd` as the client reads it: which of its declared parameters
//! carry a value, and which do not. A parameter the server declines is simply absent from the
//! packet, so "the chest does not appear" and "the chest has no position" look identical from
//! the outside -- this is how to tell them apart.

use skysaga_game::{World, WorldConfig};
use skysaga_proto::bitstream::BitReader;
use skysaga_proto::packets::SyncData;
use skysaga_world::{default_entities_path, EntityDefinitions};

fn main() {
    let definitions = EntityDefinitions::load(default_entities_path()).expect("Entities.json");
    let config = WorldConfig::default();

    let world = World::home_island(&definitions, &config);

    let spawn = config.terrain.spawn();

    println!("player spawn voxel   {spawn:?}");
    println!("entities in world    {}", world.entities.len());

    let Some(container) = world.containers.first() else {
        println!("NO CONTAINER IN THE WORLD");

        return;
    };

    println!("container            {} (entity {})", container.name, container.id);

    let definition = definitions
        .get(&container.name)
        .expect("the container's definition");

    println!("declared parameters  {}", definition.synced_parameter_count());

    // The encoded form, as the burst carries it.
    let add = world
        .entities
        .iter()
        .find(|entity| entity.id == container.id)
        .expect("the chest is in the burst");

    let mut reader = BitReader::from_bytes(&add.sync_data.bytes());

    let sync = SyncData::decode(&mut reader, definition.synced_parameter_count())
        .expect("the chest's sync data decodes");

    println!("payload bits         {}", sync.parameters.len());
    println!();

    for index in 0..definition.synced_parameter_count() {
        let Some((component, parameter)) = definition.parameter_at(index) else {
            continue;
        };

        let present = sync.present.get(index).copied().unwrap_or(false);

        println!(
            "  [{index:>2}] {} {component}.{parameter}",
            if present { "SENT   " } else { "absent " },
        );
    }

    println!();
    println!("If `transformcomponent.position` is absent the chest is at the origin, which is");
    println!("inside the terrain and invisible -- the same shape as the Tree bug in the C#.");

}
