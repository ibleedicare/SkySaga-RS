//! The world a connecting client is told about.
//!
//! A snapshot of what the server sends during the handshake. Deliberately plain data: the
//! session state machine only reads it, so it can be built by [`World::home_island`], decoded
//! from a capture (as the tests do), or assembled by hand.

use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, MapSpec, ServerInfo};
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

    /// The GeoData Adventure this world is, by name.
    ///
    /// The client resolves the whole world from its hash, so this is what selects the scene.
    /// `CharacterCustomiser_Adventure` is the character creator's own world -- the deck with
    /// banners -- whose biome is `CharacterCustomise`.
    pub adventure: String,

    /// How many voxels above the surface the player spawns.
    ///
    /// The creator camera sits at an offset from the player, so too little clearance puts the
    /// camera inside terrain and the character behind it. Measured: at 3 voxels the camera is
    /// buried in a sand bank and the character is not in frame; at 25 both the character and
    /// the island behind it render.
    pub spawn_clearance: i32,

    /// The adventure's nested WorldType: 1 = home, 2 = quest, 3 = PVP, 5 = sandbox, and 0 for
    /// the character customiser. Only a home world may be edited, which is what lets crafting
    /// stations be placed.
    pub world_type: u32,
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
            spawn_clearance: 25,
            adventure: "Home_Island_Adventure".to_owned(),
            world_type: 1,
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
                adventure: Some(skysaga_core::name_hash(&config.adventure)),
                map_header_seed: 0,
                // Only a home world may be edited. Items with IsLockedToHomeIsland refuse to
                // be placed anywhere else.
                is_home_world: config.world_type == 1,
                is_my_world: config.world_type == 1,
                chat_host: config.chat_host.clone(),
                chat_port: config.chat_port,

                // --- read only by build 36731 ---------------------------------------------
                //
                // The 2017 struct carries the owner as a binary uuid rather than a string, and
                // adds a world and a server uuid. They are sent as absent when we have none:
                // an all-zero uuid is a *value*, and the client cannot tell it from a real one.
                owner_uuid: skysaga_proto::types::uuid_to_wire_bytes(&config.owner_guid),
                world_uuid: None,
                server_uuid: None,
                max_users: 32,
                min_users_required_to_play: 0,
                game_mode_entity_id: 0,
                is_opened_to_matchmaking: false,
            },

            map: MapDefinition {
                size_chunks: [config.terrain.size_chunks as u32; 3],
                biome: Some(skysaga_core::name_hash(&config.biome)),
                game_mode: 1,

                // --- read only by build 36731 ---------------------------------------------
                //
                // Real GeoData indices, derived from `Adventures[82]` in build 36731's own
                // GeoData rather than chosen by hand. Index 0 is the client's "none" sentinel,
                // so the all-zero map it used to get named nothing and could not be resolved.
                //
                // `adventure_type` and `map_file_name` stay overridable so the remaining
                // unknowns can be bisected without a rebuild.
                spec: map_spec_b36731(config.terrain.seed as u32),
                adventure_type: std::env::var("SKYSAGA_MAP_ADVENTURE_TYPE").unwrap_or_default(),
                map_file_name: std::env::var("SKYSAGA_MAP_FILENAME").unwrap_or_default(),
                ..MapDefinition::default()
            },

            chunks: terrain_chunks(&config.terrain),
            entities,
            player_entity_id,
        }
    }
}

/// Position units are 1/32 of a voxel: a chunk origin is `chunkCoord * 32` voxels, and a
/// voxel is 32 units across. Voxel coordinates must be scaled by this before they go on the
/// wire.
///
/// Sending raw voxel coordinates puts an entity at 1/32 of its intended position -- for a
/// spawn at the middle of the island, that is voxel 2, in the corner and *inside* the ground.
/// An entity buried in terrain renders unlit, which is what a black character means.
pub const POSITION_SCALE: u32 = 32;

/// Everything the player entity replicates.
fn player_components(config: &WorldConfig) -> Vec<Component> {
    use skysaga_world::*;

    let spawn = config.terrain.spawn();
    // spawn() already includes 3 voxels of clearance; add any extra on top.
    let spawn = (spawn.0, spawn.1 + config.spawn_clearance - 3, spawn.2);

    vec![
        Component::PlayerAspects(PlayerAspectsComponent {
            // Without these the player cannot build, which is most of the game.
            can_edit_map: true,
            can_create_devices: true,
            can_damage_entities: true,
            can_damage_devices: true,
            ..Default::default()
        }),
        // Two slots, as the C# seeds. The count is at the list's default of 2, so this takes
        // the escape path and is not the same bits as an empty list.
        Component::CraftingDropSlots(CraftingDropSlotsComponent { slots: vec![0, 0] }),
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
            position: [
                spawn.0 as u32 * POSITION_SCALE,
                spawn.1 as u32 * POSITION_SCALE,
                spawn.2 as u32 * POSITION_SCALE,
            ],
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

/// The 2017 client's `MapSpec`, with the parts we are least sure of overridable.
///
/// The client stalls in its `LOAD_GAME_OBJECTS` stage: it parses the map, resolves it, stays
/// connected, then loads objects forever without ever asking for the world. That stage runs
/// *before* `DOWNLOAD_WORLD` in the client's own stage list, so it is not waiting for terrain,
/// and pushing terrain early makes it disconnect instead. Whatever it cannot finish loading is
/// named by this packet.
///
/// Two candidates, both switchable so one run can tell them apart without a rebuild:
///
/// - `SKYSAGA_MAPSPEC_FEATURE` replaces the feature name. It defaults to the adventure's
///   `RootFeature`, `Home_Island_World`, which is the one value here we effectively invented:
///   it appears nowhere in 36731's `GeoData.json`, which has no `Features` table at all, so the
///   client must resolve it from resources and may never find it. Set it empty to send no name.
/// - `SKYSAGA_MAPSPEC_FILL=1` fills the slots the adventure leaves empty with the home island's
///   own entries instead of the "none" sentinel: region, palette, the three creature sets and
///   the terrain generator. "None" is the faithful reading of the adventure, but a world may
///   still need them before it can build anything.
fn map_spec_b36731(seed: u32) -> MapSpec {
    let mut spec = MapSpec::home_island_b36731(seed);

    if let Ok(feature) = std::env::var("SKYSAGA_MAPSPEC_FEATURE") {
        spec.searchable_string_b = feature;
    }

    if std::env::var("SKYSAGA_MAPSPEC_FILL").as_deref() == Ok("1") {
        // Wire index is the GeoData position plus one: index 0 is the client's "none".
        spec.searchable[1] = 1; // region              Regions[0] DesertRegion1
        spec.searchable[4] = 23; // palette            BiomePalettes[22] _Forest_HomeIsland
        spec.searchable[5] = 30; // featureCreatureSet
        spec.searchable[6] = 30; // terrainCreatureSet CreatureSets[29] _HomeIsland_Desert
        spec.searchable[7] = 30; // caveCreatureSet
        spec.searchable[12] = 4; // terrainGenerator   TerrainGenerators[3] Desert
    }

    spec
}
