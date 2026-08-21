//! The world a connecting client is told about.
//!
//! A snapshot of what the server sends during the handshake. Deliberately plain data: the
//! session state machine only reads it, so it can be built by [`World::home_island`], decoded
//! from a capture (as the tests do), or assembled by hand.

use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, ServerInfo};
use skysaga_world::terrain::CHUNK_SIZE;
use skysaga_world::{
    Component, Entity, EntityDefinitions, HealthComponent, InteractionComponent,
    InventoryComponent, OwnerComponent, PhysicsComponent, PickupComponent, PlayerNameComponent,
    TerrainGenerator, TimeOfDayComponent, TransformComponent, VoxelLinkComponent,
};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct World {
    pub server_info: ServerInfo,
    pub map: MapDefinition,

    /// One per chunk of terrain. `BeginSync` announces the count.
    pub chunks: Vec<ChunkSync>,

    /// Every entity the client should know about on arrival, the player included.
    pub entities: Vec<EntityAdd>,

    /// Which of `entities` is this connection's player. Sent as `SetClientEntity`.
    pub player_entity_id: u32,
}

/// Knobs the home island is built with.
#[derive(Debug, Clone)]
pub struct WorldConfig {
    pub owner_guid: String,
    pub owner_name: String,
    pub biome: String,
    pub chat_host: String,
    pub chat_port: u16,
    pub terrain: TerrainGenerator,

    /// Frozen time of day, over a 65536-tick cycle.
    ///
    /// The C# comments this as "65536 = full cycle, so 32768 is midday", but 32768 renders
    /// with low amber light and hard shadows -- which looks like dusk, not noon. That would
    /// mean the cycle starts at dawn rather than midnight, making midday nearer 16384. Left
    /// configurable rather than asserted either way; SKYSAGA_TIME_OF_DAY sets it.
    pub time_of_day: u32,

    /// When false the clock runs, which leaves the world dark half the time.
    pub fixed_time_of_day: bool,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            owner_guid: String::new(),
            // The C#'s defaults, so a comparison against its capture is like for like.
            owner_name: "Adventurer".to_owned(),
            biome: "Desert".to_owned(),
            chat_host: "127.0.0.1".to_owned(),
            chat_port: 4444,
            terrain: TerrainGenerator::default(),
            time_of_day: 65536 / 2,
            fixed_time_of_day: true,
        }
    }
}

/// The animals and props the C# seeds: name, position, and half-hearts.
///
/// The Tree is placed where the C# *intends* rather than where it lands. Server.cs assigns
/// its position through TryGetComponent<SmoothedTransformComponent>, but Tree binds `position`
/// to plain `transformcomponent`, so the assignment silently does nothing and the tree spawns
/// at the origin -- confirmed by decoding the capture, which has it at [0, 0, 0]. Reproducing
/// that would mean shipping a known bug; the divergence is asserted in tests/home_island.rs.
///
/// Only the Sheep is given health -- Server.cs assigns HalfHearts on that one alone and
/// leaves the rest at zero. Applying it to all of them changes one byte of every other
/// animal's payload, which is how this was caught.
const PROPS: &[(&str, [u32; 3], u32)] = &[
    ("Sheep", [2000, 70, 629], 50),
    ("Bear", [2200, 70, 629], 0),
    ("Chicken", [2400, 70, 629], 0),
    ("Goat", [2600, 70, 629], 0),
    ("Knight", [2800, 70, 629], 0),
    ("Monkey", [3000, 70, 629], 0),
    ("Tree", [3000, 70, 1000], 0),
];

impl World {
    /// Build the home island: terrain, the seeded entities, and a player.
    ///
    /// Entity ids are assigned in creation order starting at 1, so the player is last. The
    /// client is told which id is its own by `SetClientEntity`, so the numbering is ours to
    /// choose — it does not have to match the C#'s.
    pub fn home_island(definitions: &EntityDefinitions, config: &WorldConfig) -> Self {
        let mut entities = Vec::new();
        let mut next_id = 1u32;

        let mut add = |name: &str, components: Vec<Component>| -> Option<u32> {
            let Some(definition) = definitions.get(name) else {
                // A name the data file does not define. Report it rather than silently
                // shipping a world with a hole in it.
                warn!(entity = name, "not defined in Entities.json; skipping");
                return None;
            };

            let id = next_id;
            next_id += 1;

            entities.push(Entity::new(id, components).to_entity_add(definition));

            Some(id)
        };

        add(
            "Airship",
            vec![
                Component::Transform(TransformComponent {
                    position: [2000, 70, 629],
                    ..Default::default()
                }),
                Component::Interaction(InteractionComponent::default()),
                Component::Owner(OwnerComponent::default()),
                Component::Pickup(PickupComponent::default()),
                Component::VoxelLink(VoxelLinkComponent::default()),
            ],
        );

        add(
            "TimeOfDay",
            vec![Component::TimeOfDay(TimeOfDayComponent {
                start_time_of_day: config.time_of_day,
                fixed_time_of_day: config.fixed_time_of_day,
                day_night_cycle_duration: 64,
                time_stretch: 64,
                time_of_day_offset: 0,
                real_world_start_time: 0,
            })],
        );

        for (name, position, half_hearts) in PROPS {
            add(
                name,
                vec![
                    Component::SmoothedTransform(TransformComponent {
                        position: *position,
                        ..Default::default()
                    }),
                    Component::Transform(TransformComponent {
                        position: *position,
                        ..Default::default()
                    }),
                    Component::Health(HealthComponent {
                        half_hearts: *half_hearts,
                        ..Default::default()
                    }),
                    Component::Inventory(InventoryComponent::default()),
                    Component::CharacterPhysics(PhysicsComponent::default()),
                    Component::PlayerName(PlayerNameComponent::default()),
                ],
            );
        }

        let player_entity_id = add("Player", player_components(config)).unwrap_or(0);

        Self {
            server_info: ServerInfo {
                owner_guid: config.owner_guid.clone(),
                owner_name: config.owner_name.clone(),
                biome: config.biome.clone(),
                adventure: None,
                map_header_seed: 0,
                // You own your home island; that is what lets crafting stations be placed.
                is_home_world: true,
                is_my_world: true,
                chat_host: config.chat_host.clone(),
                chat_port: config.chat_port,
            },

            map: MapDefinition {
                size_chunks: [config.terrain.size_chunks as u32; 3],
                biome: Some(skysaga_core::name_hash("Sky_Island")),
                game_mode: 1,
            },

            chunks: terrain_chunks(&config.terrain),
            entities,
            player_entity_id,
        }
    }
}

/// Everything the player entity replicates.
fn player_components(config: &WorldConfig) -> Vec<Component> {
    use skysaga_world::*;

    let spawn = config.terrain.spawn();

    vec![
        Component::PlayerAspects(PlayerAspectsComponent {
            // Without these the player cannot build, which is most of the game.
            can_edit_map: true,
            can_create_devices: true,
            can_damage_entities: true,
            can_damage_devices: true,
            ..Default::default()
        }),
        Component::CraftingDropSlots(CraftingDropSlotsComponent::default()),
        Component::FeatureUnlock(FeatureUnlockComponent::default()),
        Component::Health(HealthComponent {
            half_hearts: 20,
            whole_hearts: 10,
            ..Default::default()
        }),
        Component::Inventory(InventoryComponent {
            max_inventory_slots: 36,
            ..Default::default()
        }),
        Component::CharacterPhysics(PhysicsComponent::default()),
        Component::MailBox(MailBoxComponent::default()),
        Component::Owner(OwnerComponent {
            owner: config.owner_guid.clone(),
        }),
        Component::PlayerName(PlayerNameComponent {
            player_name: config.owner_name.clone(),
        }),
        Component::SmoothedTransform(TransformComponent {
            position: [spawn.0 as u32, spawn.1 as u32, spawn.2 as u32],
            ..Default::default()
        }),
        Component::UseEntity(UseEntityComponent::default()),
        Component::Wallet(WalletComponent::default()),
    ]
}

/// Every solid chunk of the island, as `ChunkSync` packets.
///
/// Chunks that generate as entirely air are not sent at all — the C# skips them, and the
/// count in `BeginSync` has to match what actually follows.
fn terrain_chunks(terrain: &TerrainGenerator) -> Vec<ChunkSync> {
    let mut chunks = Vec::new();

    for x in 0..terrain.size_chunks {
        for z in 0..terrain.size_chunks {
            let Some(data) = terrain.chunk(x, 0, z) else {
                continue;
            };

            chunks.push(ChunkSync {
                coords: [x as u32, 0, z as u32],
                data1: Some(data),
                data2: None,
                adjacent_chunks: None,
            });
        }
    }

    debug_assert!(CHUNK_SIZE == 32);

    chunks
}
