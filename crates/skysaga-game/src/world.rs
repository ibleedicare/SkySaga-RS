//! The world a connecting client is told about.
//!
//! A snapshot of what the server sends during the handshake. Deliberately plain data: the
//! session state machine only reads it, so it can be built by [`World::home_island`], decoded
//! from a capture (as the tests do), or assembled by hand.

use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, ServerInfo};
use skysaga_world::terrain::CHUNK_SIZE;
use skysaga_world::{
    Component, Entity, EntityDefinition, EntityDefinitions,
    HealthComponent, InteractionComponent,
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

    /// Where the player sits in `entities`, so its burst entry can be replaced.
    pub player_index: usize,

    /// The GeoData Adventure this world is, by name.
    ///
    /// `server_info` carries only its hash, which is what the client resolves the scene from.
    /// The name is kept alongside for anything that has to report what is being served.
    pub adventure: String,

    /// Where a client is sent once it has finished creating its homeworld. See
    /// [`WorldConfig::public_ip`].
    pub transfer_ip: String,
    pub transfer_port: u16,

    /// The player entity before serialisation, with the definition needed to re-encode it.
    ///
    /// The world is built once at startup, but a character's name and appearance are not
    /// known then — they arrive over RakNet during character creation, long after this
    /// entity was first serialised. Re-encoding from the template is what lets the burst
    /// carry *this* player's character rather than the defaults the world was built with.
    ///
    /// `None` for a world decoded from a capture: a capture holds encoded `EntityAdd`s and
    /// not components, so there is nothing to re-encode *from*. Such a world replays the
    /// captured bytes verbatim, which is exactly what makes it usable as an oracle.
    pub player_template: Option<(Entity, EntityDefinition)>,

    /// `BasicInventoryItem`, for stacks created while the server runs.
    pub item_definition: Option<EntityDefinition>,
}

impl World {
    /// The player's `EntityAdd`, carrying the character in `profile`.
    ///
    /// Falls back to the template's defaults for anything the profile does not set, so a
    /// player who has never opened the creator still replicates a complete entity.
    /// The definition for a stack of items, if the data file has one.
    ///
    /// Kept because an item entity is built at runtime rather than at startup, and building
    /// one needs its definition to know which parameters to write.
    pub fn item_definition(&self) -> Option<&EntityDefinition> {
        self.item_definition.as_ref()
    }

    /// A player body for `profile`, under `entity_id`.
    ///
    /// The id is the caller's to choose: the game server allocates one per connection, which
    /// is what lets two players exist at once.
    pub fn player_body(
        &self,
        profile: &crate::CharacterProfile,
        entity_id: u32,
        inventory: &[u32],
    ) -> EntityAdd {
        self.player_entity(profile, entity_id, inventory)
            .map(|(entity, definition)| entity.to_entity_add(definition))
            .unwrap_or_else(|| {
                let mut body = self.player_entity_add(profile);

                body.id = entity_id;

                body
            })
    }

    /// The player entity itself, before serialisation, so it can be synced as well as added.
    pub fn player_entity(
        &self,
        profile: &crate::CharacterProfile,
        entity_id: u32,
        inventory: &[u32],
    ) -> Option<(Entity, &EntityDefinition)> {
        let (template, definition) = self.player_template.as_ref()?;

        let mut entity = template.clone();

        entity.id = entity_id;

        for component in &mut entity.components {
            match component {
                Component::CharacterCustomisation(customisation) => {
                    if let Some(appearance) = &profile.appearance {
                        customisation.customisation = appearance.clone();
                    }
                }

                Component::PlayerName(player_name) => {
                    if let Some(name) = &profile.name {
                        player_name.player_name = name.clone();
                    }
                }

                // The rucksack: entity ids of the stacks this player is carrying.
                Component::Inventory(inv) => {
                    inv.inventory_entity_list = inventory.to_vec();
                }

                _ => {}
            }
        }

        Some((entity, definition))
    }

    pub fn player_entity_add(&self, profile: &crate::CharacterProfile) -> EntityAdd {
        // A capture-built world has no template; replay what was captured.
        let Some((template, definition)) = &self.player_template else {
            return self.entities[self.player_index].clone();
        };

        let mut player = template.clone();

        for component in &mut player.components {
            match component {
                Component::CharacterCustomisation(customisation) => {
                    if let Some(appearance) = &profile.appearance {
                        customisation.customisation = appearance.clone();
                    }
                }

                Component::PlayerName(player_name) => {
                    if let Some(name) = &profile.name {
                        player_name.player_name = name.clone();
                    }
                }

                _ => {}
            }
        }

        player.to_entity_add(definition)
    }
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

    /// Where to send a client that has just created its homeworld.
    ///
    /// Character creation ends with the client idle and still connected: it has unloaded the
    /// creator's world and is waiting to be told where its own world is. This is the address
    /// `TransferToServer` carries, and it is the same one `game-conductor/retrieve` hands out
    /// over HTTP — one server, so a client is transferred back to this one.
    pub public_ip: String,
    pub game_port: u16,
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
            public_ip: "127.0.0.1".to_owned(),
            game_port: crate::server::DEFAULT_PORT,
        }
    }
}

/// How many inventory slots a player has, and the layout of them.
///
/// Read off the client UI rather than any data file: nothing in `Entities.json` or
/// `geodata.json` records the mapping, and it was resolved empirically by filling every slot
/// and reading the squares back.
///
/// ```text
///   0..1    equipment, hands
///   2..5    equipment: head, torso, legs, arms
///   6       hotbar
///   7..8    inside the count, but no square in the UI shows them
///   9..44   rucksack, 36 squares in a 6x6 grid
/// ```
pub const MAX_INVENTORY_SLOTS: u8 = 45;

/// The first slot of the rucksack proper. Anything below this is worn or held.
pub const FIRST_RUCKSACK_SLOT: usize = 9;

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

        // The player is added last, and kept un-encoded as well: its name and appearance are
        // filled in per connection, once the player has been through the creator.
        let player_definition = definitions
            .get("Player")
            .expect("Entities.json defines Player")
            .clone();

        let player_entity_id = add("Player", player_components(config)).unwrap_or(0);
        let player_index = entities.len() - 1;
        let player_template = Entity::new(player_entity_id, player_components(config));

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
            },

            map: MapDefinition {
                size_chunks: [config.terrain.size_chunks as u32; 3],
                biome: Some(skysaga_core::name_hash(&config.biome)),
                game_mode: 1,
            },

            chunks: terrain_chunks(&config.terrain),
            entities,
            player_entity_id,
            player_index,
            adventure: config.adventure.clone(),
            transfer_ip: config.public_ip.clone(),
            transfer_port: config.game_port,
            player_template: Some((player_template, player_definition)),
            item_definition: definitions.get("BasicInventoryItem").cloned(),
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
        // Sync index 19. Attached unconditionally, even before the player has chosen
        // anything: an *absent* parameter is what makes the client fall back to its built-in
        // defaults, so a default value has to be replicated rather than nothing at all. The
        // real appearance is filled in per connection by `World::player_entity_add`.
        Component::CharacterCustomisation(CharacterCustomisationComponent::default()),
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
            max_inventory_slots: MAX_INVENTORY_SLOTS,
            // Every slot present and empty. The client expects the whole list: a short one
            // leaves it with nowhere to draw, which is why an item placed in a one-element
            // list never appeared.
            inventory_entity_list: vec![0; MAX_INVENTORY_SLOTS as usize],
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
