//! The world a connecting client is told about.
//!
//! A snapshot of what the server sends during the handshake. Deliberately plain data: the
//! session state machine only reads it, so it can be built by [`World::home_island`], decoded
//! from a capture (as the tests do), or assembled by hand.

use skysaga_proto::packets::chat::Channel;
use skysaga_proto::packets::voxel::PartialChunkEditsSync;
use skysaga_proto::packets::{ChunkSync, EntityAdd, MapDefinition, ServerInfo};
use skysaga_world::geodata::{default_geodata_path, GeoData};
use skysaga_world::terrain::CHUNK_SIZE;
use skysaga_world::{
    Component, Entity, EntityDefinition, EntityDefinitions,
    HealthComponent, InteractionComponent,
    InventoryComponent, OwnerComponent, PhysicsComponent, PickupComponent, PlayerNameComponent,
    TerrainGenerator, TimeOfDayComponent, TransformComponent, VoxelLink, VoxelLinkComponent,
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

    /// The game's own tables: which block an item places, what a broken one drops, how large
    /// a stack may be.
    ///
    /// Empty for a world decoded from a capture, which carries packets rather than data. A
    /// server with an empty table cannot tell a placement from a dig, so it treats every
    /// swing as a dig -- which is wrong, but wrong in the direction that cannot duplicate
    /// items.
    pub geodata: GeoData,

    /// The chat channels this world offers.
    ///
    /// Handed out on request over RakNet; the messages themselves go over the IRC socket that
    /// `server_info` names. An empty list means the client never issues a `JOIN` and chat is
    /// silent even with the IRC server running.
    pub chat_channels: Vec<Channel>,

    /// The creatures the world seeded, un-encoded.
    pub creatures: Vec<Creature>,

    /// The containers in this world, un-encoded.
    ///
    /// Kept beside `entities` for the same reason as `player_template`: a chest's contents and
    /// its lid change while the server runs, so the burst's frozen encoding is not enough to
    /// answer with later.
    pub containers: Vec<Container>,

    /// Where a player drops in, in voxels.
    pub spawn_voxel: [u32; 3],

    /// Every entity type the data file defines.
    ///
    /// The world itself is built once, but not everything in it is: a chest spawned by a
    /// command needs its definition to know which parameters to write, and the name comes from
    /// a chat message rather than from this list. Empty for a world decoded from a capture.
    pub definitions: EntityDefinitions,
}

/// Something in the world with health: an animal, a bandit, a knight.
///
/// Kept un-encoded beside `entities` for the same reason as a container: what is replicated
/// changes while the server runs. A creature's hearts move every time it is hit, and the
/// burst's frozen encoding cannot be updated.
#[derive(Debug, Clone)]
pub struct Creature {
    pub id: u32,
    pub name: String,

    pub entity: Entity,
    pub definition: EntityDefinition,

    /// In the client's position units, so a swing can be tested against it.
    pub position: [u32; 3],

    /// Hit points at full health, resolved from the entity's own `physicalproperties`.
    ///
    /// **Not always what the burst announced.** The world seeds its animals with the hearts
    /// the C# gives them, which is a fixed 50 half-hearts on the Sheep and nothing at all on
    /// the rest -- those numbers are what the handshake oracle compares against, so they are
    /// left alone. The first hit syncs the real figure and the bar corrects itself.
    pub max_health: u32,
}

/// A container in the world: a chest, and later a mailbox or a crafting station.
#[derive(Debug, Clone)]
pub struct Container {
    pub id: u32,
    pub name: String,

    /// The entity as built, so an updated one can be re-encoded from it.
    pub entity: Entity,
    pub definition: EntityDefinition,

    pub slots: usize,

    /// Whether E closes it as well as opening it.
    ///
    /// A loot chest has no close button of its own, so E is the only way to shut it and a
    /// toggle is right. Anything with an X button is re-opened instead -- see
    /// [`crate::Session`] on why.
    pub is_loot_chest: bool,
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

    /// The container with this entity id, if it is one.
    ///
    /// `None` for anything else, which is what makes "press E on a sheep" do nothing rather
    /// than open an empty window.
    pub fn container(&self, id: u32) -> Option<&Container> {
        self.containers.iter().find(|container| container.id == id)
    }

    /// The creature with this entity id, if it is one.
    ///
    /// `None` for a chest, a tree or a player, which is what keeps a swing at scenery from
    /// resolving to a hit.
    pub fn creature(&self, id: u32) -> Option<&Creature> {
        self.creatures.iter().find(|creature| creature.id == id)
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

    /// The biome **name** `ServerInfo` carries.
    ///
    /// Cosmetic: the client has no cross-reference to this string at all. It is sent because
    /// the C# sends it and the capture has it, not because anything reads it.
    pub biome: String,

    /// The biome **type** `MapDefinition` carries, which is the one that matters.
    ///
    /// The client resolves the world, its terrain and its ambience from this hash. The C#
    /// hardcodes `Sky_Island` here whatever `SKYSAGA_BIOME` says, and the captured packet
    /// agrees, so it is kept separate from [`Self::biome`] rather than derived from it.
    pub map_biome: String,
    pub chat_host: String,
    pub chat_port: u16,

    /// `type:name` pairs, comma separated. The client turns `global` into `#global`.
    pub chat_channels: String,
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

impl WorldConfig {
    /// Read the world's settings from the environment.
    ///
    /// **Both binaries must use this.** `skysaga-server` built its world from
    /// `WorldConfig::default()` and so ignored every one of these variables, while the
    /// standalone `skysaga-game` read them: setting `SKYSAGA_BIOME` or `SKYSAGA_FLAT_WORLD`
    /// against the server anyone actually runs did nothing at all, silently.
    pub fn from_env() -> Self {
        fn parse<T: std::str::FromStr>(name: &str, fallback: T) -> T {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(fallback)
        }

        let defaults = Self::default();

        Self {
            owner_name: std::env::var("SKYSAGA_PLAYER_NAME").unwrap_or(defaults.owner_name),
            adventure: std::env::var("SKYSAGA_ADVENTURE").unwrap_or(defaults.adventure),
            biome: std::env::var("SKYSAGA_BIOME").unwrap_or(defaults.biome),
            spawn_clearance: parse("SKYSAGA_SPAWN_CLEARANCE", defaults.spawn_clearance),
            world_type: parse("SKYSAGA_WORLD_TYPE", defaults.world_type),
            time_of_day: parse("SKYSAGA_TIME_OF_DAY", defaults.time_of_day),
            fixed_time_of_day: std::env::var("SKYSAGA_TIME_OF_DAY").as_deref() != Ok("cycle"),
            terrain: TerrainGenerator {
                seed: parse("SKYSAGA_WORLD_SEED", defaults.terrain.seed),
                size_chunks: parse("SKYSAGA_WORLD_CHUNKS", defaults.terrain.size_chunks),
            },
            ..defaults
        }
    }
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            owner_guid: String::new(),
            // The C#'s defaults, so a comparison against its capture is like for like.
            owner_name: "Adventurer".to_owned(),
            biome: "Desert".to_owned(),
            map_biome: "Sky_Island".to_owned(),
            chat_host: "127.0.0.1".to_owned(),
            chat_port: 4444,
            chat_channels: "0:global".to_owned(),
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

        let geodata = load_geodata();

        let mut creatures = Vec::new();

        for (name, position, half_hearts) in PROPS {
            let components = creature_components(
                *position,
                HealthComponent {
                    half_hearts: *half_hearts,
                    ..Default::default()
                },
            );

            let Some(id) = add(name, components.clone()) else {
                continue;
            };

            // The props are seeded from a fixed table, but their *health* is not in it: it
            // comes from the same lookup a spawned creature uses, so a swing at the seeded
            // Knight and a swing at one from `/mob` mean the same thing.
            let Some(definition) = definitions.get(name) else {
                continue;
            };

            let Some(max_health) = health_of(definition, &geodata) else {
                // A prop rather than a creature -- the Tree has no physical properties, so
                // nothing can say how much health it has and nothing may hit it.
                continue;
            };

            creatures.push(Creature {
                id,
                name: (*name).to_owned(),
                entity: Entity::new(id, components),
                definition: definition.clone(),
                position: *position,
                max_health,
            });
        }

        // A chest, so the world contains something a player can open.
        //
        // The C# has none either: it reaches one through its `/spawn` chat command, which is a
        // different feature. Seeding one is the smaller choice and it makes the whole container
        // path reachable -- without it, every interaction assertion is vacuous and the client
        // has nothing to press E on.
        //
        // Kept un-encoded as well as encoded, for the same reason as the player: what is in it
        // and whether its lid is shut both change while the server runs.
        let mut containers = Vec::new();

        if let Some(definition) = definitions.get(CHEST) {
            let links = definition
                .default_voxel_links()
                .into_iter()
                .map(|(offset, voxel_index)| VoxelLink {
                    x: offset[0],
                    y: offset[1],
                    z: offset[2],
                    voxel_index,
                })
                .collect();

            let components = seeded_chest_components(config, links);

            // `add` returns the id it assigned, which is also how a name the data file does
            // not define is skipped without leaving a container pointing at nothing.
            if let Some(id) = add(CHEST, components.clone()) {
                containers.push(Container {
                    id,
                    name: CHEST.to_owned(),
                    entity: Entity::new(id, components),
                    definition: definition.clone(),
                    slots: CHEST_SLOTS,
                    is_loot_chest: true,
                });
            }
        }

        // The player is added last, and kept un-encoded as well: its name and appearance are
        // filled in per connection, once the player has been through the creator.
        let player_definition = definitions
            .get("Player")
            .expect("Entities.json defines Player")
            .clone();

        let player_entity_id = add("Player", player_components(config, &geodata)).unwrap_or(0);
        let player_index = entities.len() - 1;
        let player_template = Entity::new(player_entity_id, player_components(config, &geodata));

        // The biome the client resolves the world from has to be the one holding this
        // adventure; falling back to the configured name only matters for data that has no
        // such biome, which the real tables always do.
        let map_biome = geodata
            .biome_for_adventure(&config.adventure)
            .unwrap_or(&config.map_biome)
            .to_owned();

        // The adventure carries its own kind, and the flags have to follow it: telling the
        // client a quest adventure is a home world makes it draw the home-island badge for a
        // world with no home title, which renders as a bare `%s` and no banner at all.
        let world_type = geodata
            .world_type_for_adventure(&config.adventure)
            .unwrap_or(config.world_type);

        Self {
            server_info: ServerInfo {
                owner_guid: config.owner_guid.clone(),
                owner_name: config.owner_name.clone(),
                biome: config.biome.clone(),
                adventure: Some(skysaga_core::name_hash(&config.adventure)),
                map_header_seed: 0,
                // Only a home world may be edited. Items with IsLockedToHomeIsland refuse to
                // be placed anywhere else.
                is_home_world: world_type == 1,
                is_my_world: world_type == 1,
                chat_host: config.chat_host.clone(),
                chat_port: config.chat_port,
            },

            map: MapDefinition {
                size_chunks: [config.terrain.size_chunks as u32; 3],
                // **Not `config.biome`, and not a constant either.** This is the
                // `BiomeType` the client resolves the world from, and it must be *the biome
                // that contains the adventure* named in `ServerInfo`: the client looks an
                // adventure up inside a biome, not globally.
                //
                // `Home_Island_Adventure` belongs to `Sky_Island`. Naming `Desert` here is
                // not an invalid biome -- it is a real one -- but it contains no adventures at
                // all, so the lookup failed and the client abandoned the whole resolve. The
                // island banner then drew in the wrong colour with an unresolvable title.
                //
                // The C# hardcodes `Sky_Island`, which is correct only because it always
                // serves that one adventure. Deriving it keeps the two fields consistent for
                // any adventure.
                biome: Some(skysaga_core::name_hash(&map_biome)),
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
            geodata,
            definitions: definitions.clone(),
            creatures,
            spawn_voxel: {
                let spawn = config.terrain.spawn();
                [spawn.0 as u32, spawn.1 as u32, spawn.2 as u32]
            },
            chat_channels: Channel::parse_list(&config.chat_channels),
            containers,
        }
    }
}

/// The entity seeded as the world's container.
const CHEST: &str = "Chest";

/// How many squares it holds. 25 is what the live session that solved chests used.
pub const CHEST_SLOTS: usize = 25;

/// The clearance `TerrainGenerator::spawn` already builds into the height it returns.
///
/// Subtracting it gets back to the surface, which is where something standing on the ground
/// belongs.
const SPAWN_CLEARANCE_VOXELS: u32 = 3;

/// Everything the chest replicates.
///
/// `links` are the cells it occupies, read from its own entry in `Entities.json`.
fn seeded_chest_components(config: &WorldConfig, links: Vec<VoxelLink>) -> Vec<Component> {
    let spawn = config.terrain.spawn();

    // Beside the player rather than on top of it: a container inside the spawn point is
    // reachable but not visible, which reads as "the chest did not spawn".
    //
    // **On the ground, not at the spawn height.** `spawn()` already includes three voxels of
    // clearance so the player drops in rather than starting inside terrain; using that height
    // for a chest leaves it hanging in the air three voxels up, which is its own kind of "the
    // chest is not there".
    let position = [
        (spawn.0 as u32 + 2) * POSITION_SCALE,
        (spawn.1 as u32 - SPAWN_CLEARANCE_VOXELS + 1) * POSITION_SCALE,
        (spawn.2 as u32 + 2) * POSITION_SCALE,
    ];

    container_components(position, links, CHEST_SLOTS)
}

/// Everything a container replicates, wherever it is and however big it is.
///
/// Shared by the chest the world seeds and any spawned by a command, so the two cannot drift
/// -- which matters because two of these values are the difference between a chest that is
/// there and one that is invisible. See `size` and the voxel link below.
pub fn container_components(
    position: [u32; 3],
    links: Vec<VoxelLink>,
    slots: usize,
) -> Vec<Component> {
    vec![
        Component::Transform(TransformComponent {
            position,
            // **One, not zero.** `size` has no default in the data file, so an unset one is
            // [0, 0, 0] and the chest renders as nothing at all -- present in the burst,
            // interactable in principle, and invisible. The C# sets it explicitly for the
            // same reason.
            size: [1, 1, 1],
            ..Default::default()
        }),
        Component::Interaction(InteractionComponent {
            enabled: true,
            is_loot_chest: true,
            // False, and it must stay false to open. This is the CLOSE signal: the client's
            // open path fires only while it is clear, and its close path on the rising edge.
            has_been_opened: false,
            owner_only: false,
            allow_multiple_users: true,
        }),
        Component::Inventory(InventoryComponent {
            max_inventory_slots: slots as u8,
            // Every square present and empty, as for the player: a short list leaves the
            // client nowhere to draw.
            inventory_entity_list: vec![0; slots],
            ..Default::default()
        }),
        Component::Owner(OwnerComponent::default()),
        Component::Pickup(PickupComponent::default()),
        // **What puts the chest in the world grid rather than floating in front of it.**
        // Every entity declaring `clientinteractioncomponent` also declares this, and an empty
        // list declines the parameter -- so a chest sent without it is not part of the terrain.
        // The shape is per entity, which is why it is read rather than hardcoded.
        Component::VoxelLink(VoxelLinkComponent {
            voxels: links,
            can_replace_voxels_of_entity_id: 0,
        }),
    ]
}

/// Everything a placed device replicates: an anvil, a forge, a camp fire.
///
/// The same four components a chest needs -- transform, interaction, owner, pickup, voxel links
/// -- and a crafting queue where the entity has one. What it does *not* have is an inventory: a
/// station is not a container, and its window is drawn from `craftingslots` instead.
///
/// `max_crafting_slots` comes from the entity's own `maxcraftingslots` default, so an `Anvil`
/// gets three and something that is not a station gets no crafting component at all. A
/// parameter an entity does not declare is never sent, so the component costs nothing where it
/// is not wanted -- but leaving it off entirely means a station whose queue can never be shown.
pub fn device_components(
    position: [u32; 3],
    links: Vec<VoxelLink>,
    max_crafting_slots: Option<u8>,
) -> Vec<Component> {
    let mut components = vec![
        Component::Transform(TransformComponent {
            position,
            // One, not zero: an unset size renders as nothing at all. See the container.
            size: [1, 1, 1],
            ..Default::default()
        }),
        Component::Interaction(InteractionComponent {
            enabled: true,
            // **Not a loot chest.** The flag decides whether E toggles the window shut, and a
            // station has an X button of its own -- so a toggle would leave the server a press
            // out of phase the moment the player closed the panel with the mouse.
            is_loot_chest: false,
            has_been_opened: false,
            owner_only: false,
            allow_multiple_users: true,
        }),
        Component::Owner(OwnerComponent::default()),
        Component::Pickup(PickupComponent::default()),
        // What puts the device in the world grid rather than floating in front of it. An Anvil
        // is twelve linked cells, a Camp_Fire one, and the shape is read from the entity.
        Component::VoxelLink(VoxelLinkComponent {
            voxels: links,
            can_replace_voxels_of_entity_id: 0,
        }),
    ];

    if let Some(max_slots) = max_crafting_slots {
        components.push(Component::Crafting(skysaga_world::CraftingComponent {
            slots: Vec::new(),
            max_slots,
        }));
    }

    components
}

/// Everything a creature replicates, wherever it stands and however healthy it is.
///
/// Shared by the props the world seeds and anything `/mob` puts down, so the two cannot drift.
/// The component set is the C#'s: a creature is a transform, a health bar, an inventory to
/// loot, a physics body and a name plate.
pub fn creature_components(position: [u32; 3], health: HealthComponent) -> Vec<Component> {
    vec![
        Component::SmoothedTransform(TransformComponent {
            position,
            ..Default::default()
        }),
        Component::Transform(TransformComponent {
            position,
            ..Default::default()
        }),
        Component::Health(health),
        Component::Inventory(InventoryComponent::default()),
        Component::CharacterPhysics(PhysicsComponent::default()),
        Component::PlayerName(PlayerNameComponent::default()),
    ]
}

/// How much health this entity type has, from its own `physicalproperties` default.
///
/// `None` for anything that declares none -- a tree, a barrel -- which is the test for
/// "can this be fought at all". Nothing in `entities.json` states a hit point count directly.
pub fn health_of(definition: &EntityDefinition, geodata: &GeoData) -> Option<u32> {
    geodata.health_for(definition.physical_properties()?)
}

/// Position units are 1/64 of a voxel. Voxel coordinates must be scaled by this before they go
/// on the wire.
///
/// Sending raw voxel coordinates puts an entity at 1/64 of its intended position -- for a
/// spawn at the middle of the island, that is voxel 1, in the corner and *inside* the ground.
/// An entity buried in terrain renders unlit, which is what a black character means.
///
/// # Why 64 and not 32
///
/// It was 32, chosen because it is the chunk size and because it is visibly better than
/// sending raw voxels. Nothing confirmed it, and `combat-and-health.md` read the same fields
/// as 1/64 throughout. The client settles it four times over:
///
/// * `FUN_0074a860` multiplies each world float by `DAT_00c61f28` on its way into
///   `EntityMoved`'s position, and that constant is `64.0`;
/// * `FUN_0073d7e0`, which logs the same field in the same RPC, multiplies it back by
///   `DAT_00cd3e80`, which is `1/64`. The angle logger beside it uses `1/32` on the yaw, so the
///   two scales are distinguished within one function;
/// * the component parameter getter/setter pairs agree -- `FUN_008f8270` and `FUN_008dc8d0`
///   write with `64.0`, `FUN_008f83b0` and `FUN_008dc950` read with `1/64`;
/// * and it is the reading under which the melee distances a real fight produced are a melee
///   range. See `combat::tests::the_separations_a_real_fight_produced_are_all_believed`.
///
/// At 32 every entity the server placed sat at half its intended voxel, and every distance the
/// server computed from a client position was twice what the client meant.
pub const POSITION_SCALE: u32 = 64;

/// The rank every job is seeded at.
///
/// Progression is not modelled: nothing awards experience, so a rank the player could not
/// raise would be a permanent lock rather than a goal. The reversing notes name 25 as
/// "tutorial complete", and the starting recipes need at most 21.
const FULL_JOB_RANK: u8 = 25;

/// Everything the player entity replicates.
fn player_components(config: &WorldConfig, geodata: &GeoData) -> Vec<Component> {
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
        // **Every job at full rank.** A recipe is gated on RequiredJob and RequiredJobRank,
        // and the client refuses to craft anything the player has not ranked up to, however
        // well the recipe book says it is known. The starting recipes reach Tutorial 21, so
        // anything short of the top leaves some of them locked with no way to earn a rank:
        // nothing in the server awards experience yet. Twenty-five is the tutorial-complete
        // rank the reversing notes name.
        Component::JobRank(JobRankComponent {
            jobs: geodata
                .jobs()
                .iter()
                .map(|job| JobRank {
                    name: skysaga_core::name_hash(job),
                    rank: FULL_JOB_RANK,
                    experience: 0,
                    experience_to_next: 0,
                })
                .collect(),
        }),
        // **The quest log renders from this parameter, so an empty list still has to go out.**
        // Absent is not the same as empty: with no list the client has nothing to hang rows or
        // click targets on, and the panel draws its frame and then sits inert. Six bits buys a
        // working panel. The contents are the session's, folded in by `entity_now`.
        Component::TodoList(TodoListComponent::default()),
        // One slot, which is what the data gives a player. Hand crafting is a station like
        // any other; this component is what makes the player one.
        Component::Crafting(CraftingComponent {
            slots: Vec::new(),
            max_slots: 1,
        }),
        // Two slots, as the C# seeds. That is exactly the list's default, which is the one
        // count-optimised boundary this entity actually lands on: a clear escape bit and no
        // 32-bit count.
        Component::CraftingDropSlots(CraftingDropSlotsComponent { slots: vec![0, 0] }),
        // **Without this the hand-crafting panel has no category tabs at all.** The client
        // builds the tab strip from the recipes the player knows, so an absent or empty book
        // is a panel with nothing in it -- which reads as "crafting is not implemented"
        // rather than as a missing parameter.
        Component::RecipeBook(RecipeBookComponent {
            recipes: geodata
                .starting_recipes()
                .iter()
                .map(|recipe| recipe.id())
                .collect(),
            scrolls_used: 0,
        }),
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

/// Read `geodata.json`, or carry on without it.
///
/// A missing or unreadable file is reported and then tolerated rather than fatal: the world
/// itself is built from `Entities.json`, and a server that will not start because it cannot
/// say which block "Stone" places is worse than one where placing does not work yet.
fn load_geodata() -> GeoData {
    let path = default_geodata_path();

    match GeoData::load(&path) {
        Ok(geodata) => {
            tracing::info!(
                voxels = geodata.voxel_count(),
                path = %path.display(),
                "read the geodata tables",
            );

            geodata
        }

        Err(error) => {
            warn!(%error, "no geodata; placing blocks will not work");

            GeoData::default()
        }
    }
}

impl World {
    /// What block stands at a voxel, as the world was generated.
    ///
    /// Read out of the chunk that was sent to the client rather than regenerated from the
    /// terrain function: the generator lives on the config and the world does not keep it, and
    /// reading the sent bytes means the server cannot disagree with what the player is looking
    /// at. Anything the session has since edited is **not** here; see `Session::material_at`.
    ///
    /// Air for a voxel outside the world, or in one of the all-air chunks that are never sent.
    /// That is the same answer the client would give and it makes an out-of-range dig a
    /// no-op rather than an error to handle.
    pub fn material_at(&self, chunk: [u32; 3], voxel: [u32; 3]) -> u8 {
        let Some(sync) = self.chunks.iter().find(|sync| sync.coords == chunk) else {
            return PartialChunkEditsSync::AIR;
        };

        let Some(data) = sync.data1.as_ref() else {
            return PartialChunkEditsSync::AIR;
        };

        if voxel.iter().any(|axis| *axis as usize >= CHUNK_SIZE) {
            return PartialChunkEditsSync::AIR;
        }

        // `data[0]` is the format byte, so the voxels start at 1. The axis order is the one
        // the generator writes: y is the slowest, then z, then x.
        let index = 1
            + voxel[1] as usize * CHUNK_SIZE * CHUNK_SIZE
            + voxel[2] as usize * CHUNK_SIZE
            + voxel[0] as usize;

        data.get(index)
            .copied()
            .unwrap_or(PartialChunkEditsSync::AIR)
    }

    /// The middle of a voxel, in the client's position units.
    ///
    /// Where something belongs when it belongs *in* a block rather than at a corner: a drop
    /// from a dug block, most obviously. An entity transform sits at the entity's feet, so the
    /// half-voxel lift is the same one creature loot uses to keep a pickup out of the ground.
    pub fn voxel_centre(chunk: [u32; 3], voxel: [u32; 3]) -> [u32; 3] {
        let mut centre = [0; 3];

        for (axis, out) in centre.iter_mut().enumerate() {
            let world_voxel = chunk[axis] * CHUNK_SIZE as u32 + voxel[axis];

            *out = world_voxel * POSITION_SCALE + POSITION_SCALE / 2;
        }

        centre
    }

    /// The corner of a voxel, in the client's position units.
    ///
    /// **Where a placed entity goes**, and deliberately not [`Self::voxel_centre`]. The client
    /// resolves each linked cell as `transform + (offset + 0.5)` rotated by yaw, so the half
    /// voxel is already in the link; adding it here as well would put a one-cell device half a
    /// block into the next one. Confirmed by the live C# placement recorded in
    /// `documentations/device-placement.md`: voxel `(13, 17, 19)` became position
    /// `(832, 1088, 1216)`, which is exactly `worldVoxel * 64`.
    pub fn voxel_corner(chunk: [u32; 3], voxel: [u32; 3]) -> [u32; 3] {
        let mut corner = [0; 3];

        for (axis, out) in corner.iter_mut().enumerate() {
            *out = (chunk[axis] * CHUNK_SIZE as u32 + voxel[axis]) * POSITION_SCALE;
        }

        corner
    }

    /// Where a player drops in, in the client's position units.
    ///
    /// Used when something has to be placed before the client has said where it is.
    pub fn spawn_position(&self) -> [u32; 3] {
        let spawn = self.spawn_voxel;

        [
            spawn[0] * POSITION_SCALE,
            spawn[1] * POSITION_SCALE,
            spawn[2] * POSITION_SCALE,
        ]
    }
}
