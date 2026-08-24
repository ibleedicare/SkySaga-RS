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
    ResourcePickupComponent,
    TerrainGenerator, TimeOfDayComponent, TransformComponent, VoxelLink, VoxelLinkComponent,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

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

    /// `DurableInventoryItem`, for the ones that wear out.
    ///
    /// A tool is a different entity from a stack of dirt: it carries a
    /// `clientdurabilitycomponent`, and the repair square's own test is whether the item it is
    /// handed resolves one. See [`crate::durable_items_enabled`].
    pub durable_item_definition: Option<EntityDefinition>,

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

    /// The half of the world that changes while the server runs.
    ///
    /// # Why this is behind a lock rather than `&mut World`
    ///
    /// A block one player breaks is broken for everybody, so this state cannot live on a
    /// session. Passing `&mut World` into `Session::handle` would say so in the type, at the
    /// cost of every call site and of borrowing the world mutably while iterating sessions.
    /// The game loop is a single thread draining one packet at a time, so the lock is never
    /// contended and the sessions keep taking `&World`.
    ///
    /// Shared rather than copied on clone: a cloned world is the same world.
    /// Use the methods rather than this: [`World::block_at`], [`World::set_block`] and
    /// [`World::take_unsaved_edits`]. It is public only so a world can be built by hand, as the
    /// capture-backed tests do.
    pub changes: Arc<Mutex<WorldChanges>>,
}

/// What has happened to the world since it was built. Opaque on purpose: it is reached
/// through `World`, which keeps the unsaved queue and the block map in step.
#[derive(Debug, Default)]
pub struct WorldChanges {
    /// `(chunk, voxel)` to the block that stands there now.
    voxels: HashMap<([u32; 3], [u32; 3]), u8>,

    /// Edits nobody has written down yet, oldest first.
    ///
    /// The world cannot reach the store: it is a value, and persistence lives at the edge. So
    /// it keeps a queue and the game loop drains it, the same shape as a session's
    /// notifications.
    unsaved: Vec<VoxelEdit>,

    /// Anvils, chests and barrels players have put down, oldest first.
    devices: Vec<Device>,

    /// Placements nobody has written down yet. Drained like `unsaved`.
    unsaved_devices: Vec<PlacedDevice>,

    /// Creatures that appeared while the server ran, beside the ones the island seeded.
    creatures: Vec<Creature>,

    /// Hit points taken off each creature, by entity id.
    ///
    /// Damage rather than remaining health, so a creature needs no mutable copy: what is left
    /// is its own maximum minus this.
    damage: HashMap<u32, u32>,

    /// Items lying on the floor, oldest first.
    drops: Vec<FloorDrop>,

    /// Creatures somebody has already killed.
    ///
    /// Kept rather than deleted, because "this one is dead" has to outlive the entity: a
    /// joiner must not be sent it, and a second player must not be able to kill it again.
    dead: HashSet<u32>,
}

/// Something a player put in the world: a station, a decoration, a mailbox.
///
/// On the world rather than the session that placed it, for the same two reasons as a block:
/// another player must be able to see it, and it must still be there tomorrow.
///
/// The **definition is not held here.** It is looked up from [`World::definitions`] by name,
/// which lives as long as the world does, so a device can be handed out with a borrowed
/// definition even though the device itself comes out of a lock.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: u32,
    pub name: String,

    /// The entity as built, so it can be re-encoded with its queue filled in.
    pub entity: Entity,
}

/// A device as it is written down: what it is, and where it stands.
///
/// Everything else is derived. The components come from `Entities.json` and the entity id is
/// minted per run, so storing either would be storing a fact about this process rather than
/// about the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedDevice {
    pub name: String,

    /// In the client's position units, 1/64 of a voxel.
    pub position: [u32; 3],
}

/// An item lying on the floor, as the world knows it.
///
/// **Two entities and a fact.** The `Pickup` is what the client fires at and the stack is what
/// ends up in a square; both ids belong to the world, because a drop one player makes has to be
/// nameable by every other session. The item and count are kept beside them so a session that
/// never saw the drop happen can build the stack itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloorDrop {
    /// The `Pickup` entity the client fires `ResourcePickupAction` at.
    pub pickup: u32,

    /// The `BasicInventoryItem` it points at, which is what ends up in a slot.
    pub stack: u32,

    /// The item's name hash.
    pub item: u32,
    pub count: u32,

    /// Where it is lying, in position units of 1/64 of a voxel.
    pub position: [u32; 3],
}

/// One block, changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelEdit {
    pub chunk: [u32; 3],
    pub voxel: [u32; 3],
    pub material: u8,
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

    /// The definition for a stack that wears out, if the data file has one.
    pub fn durable_item_definition(&self) -> Option<&EntityDefinition> {
        self.durable_item_definition.as_ref()
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
            durable_item_definition: definitions.get("DurableInventoryItem").cloned(),
            geodata,
            definitions: definitions.clone(),
            changes: Arc::new(Mutex::new(WorldChanges::default())),
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
/// The components of an item lying on the floor.
///
/// Built in two places -- where a drop is made, and in the burst that tells a joiner about one
/// made before they arrived -- and they have to agree, or the same pile is a different entity
/// on two screens.
pub fn pickup_components(stack: u32, at: [u32; 3]) -> Vec<Component> {
    vec![
        Component::ResourcePickup(ResourcePickupComponent::at(stack, at)),
        // `size` is the only thing the transform contributes here, and an unset one is
        // [0,0,0] -- present, collectable, and invisible.
        Component::Transform(TransformComponent {
            size: [1, 1, 1],
            ..Default::default()
        }),
    ]
}

/// Where an entity stands, out of its own transform.
fn position_of(entity: &Entity) -> Option<[u32; 3]> {
    entity.components.iter().find_map(|component| match component {
        Component::Transform(transform) => Some(transform.position),
        _ => None,
    })
}

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
    /// What block stands at a voxel **now**: what players have done to it, or the terrain.
    ///
    /// This is the one to ask. [`Self::material_at`] answers what the generator produced, which
    /// is only the same thing until somebody digs.
    pub fn block_at(&self, chunk: [u32; 3], voxel: [u32; 3]) -> u8 {
        if let Some(material) = self.changes.lock().expect("world lock").voxels.get(&(chunk, voxel))
        {
            return *material;
        }

        self.material_at(chunk, voxel)
    }

    /// Put a block somewhere, for everybody.
    ///
    /// Queues the edit for storage as well: see [`Self::take_unsaved_edits`].
    pub fn set_block(&self, chunk: [u32; 3], voxel: [u32; 3], material: u8) {
        let mut changes = self.changes.lock().expect("world lock");

        changes.voxels.insert((chunk, voxel), material);

        changes.unsaved.push(VoxelEdit {
            chunk,
            voxel,
            material,
        });
    }

    /// Every block a player has changed, for rebuilding a world or writing one down.
    pub fn block_edits(&self) -> Vec<VoxelEdit> {
        let changes = self.changes.lock().expect("world lock");

        let mut edits: Vec<VoxelEdit> = changes
            .voxels
            .iter()
            .map(|((chunk, voxel), material)| VoxelEdit {
                chunk: *chunk,
                voxel: *voxel,
                material: *material,
            })
            .collect();

        // Sorted so that two servers holding the same world report it identically, which is
        // what makes a round-trip test an equality rather than a set comparison.
        edits.sort_by_key(|edit| (edit.chunk, edit.voxel));

        edits
    }

    /// Put back edits loaded from storage, without queueing them to be written again.
    pub fn restore_block_edits(&self, edits: &[VoxelEdit]) {
        let mut changes = self.changes.lock().expect("world lock");

        for edit in edits {
            changes.voxels.insert((edit.chunk, edit.voxel), edit.material);
        }
    }

    /// Take the edits nobody has written down yet.
    ///
    /// Drained by the game loop each tick, exactly as a session's notifications are.
    pub fn take_unsaved_edits(&self) -> Vec<VoxelEdit> {
        std::mem::take(&mut self.changes.lock().expect("world lock").unsaved)
    }

    // --- devices ---------------------------------------------------------------------------

    /// Put a device in the world, for everybody, and queue it to be written down.
    ///
    /// The entity is built by the session that placed it, because building one needs the
    /// geodata the placement already resolved. What the world adds is that it belongs to
    /// everyone from here on.
    pub fn place_device(&self, device: Device) {
        let mut changes = self.changes.lock().expect("world lock");

        if let Some(position) = position_of(&device.entity) {
            changes.unsaved_devices.push(PlacedDevice {
                name: device.name.clone(),
                position,
            });
        }

        changes.devices.push(device);
    }

    /// Every device standing in the world.
    pub fn devices(&self) -> Vec<Device> {
        self.changes.lock().expect("world lock").devices.clone()
    }

    /// One device, by entity id.
    pub fn device(&self, id: u32) -> Option<Device> {
        self.changes
            .lock()
            .expect("world lock")
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
    }

    /// Where a device stands, in the client's position units.
    pub fn device_position(&self, id: u32) -> Option<[u32; 3]> {
        self.device(id).and_then(|device| position_of(&device.entity))
    }

    /// Take the placements nobody has written down yet.
    pub fn take_unsaved_devices(&self) -> Vec<PlacedDevice> {
        std::mem::take(&mut self.changes.lock().expect("world lock").unsaved_devices)
    }

    /// Put back devices loaded from storage.
    ///
    /// Silent, as [`Self::restore_block_edits`] is: a restore is a load rather than a change,
    /// and echoing it back would rewrite every row at every start.
    ///
    /// A name the data file no longer defines is skipped. The database outlives the code that
    /// reads it, and one unknown row must not stop a server from starting.
    pub fn restore_devices(&self, stored: &[PlacedDevice], definitions: &EntityDefinitions) {
        let mut next = self.next_entity_id();

        let mut changes = self.changes.lock().expect("world lock");

        for placed in stored {
            let Some(definition) = definitions.get(&placed.name) else {
                warn!(name = %placed.name, "a stored device names no entity; skipped");

                continue;
            };

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

            changes.devices.push(Device {
                id: next,
                name: definition.name().to_owned(),
                entity: Entity::new(
                    next,
                    device_components(placed.position, links, definition.max_crafting_slots()),
                ),
            });

            next += 1;
        }
    }

    // --- creatures -------------------------------------------------------------------------

    /// Put a creature in the world, for everybody.
    pub fn spawn_creature(&self, creature: Creature) {
        self.changes.lock().expect("world lock").creatures.push(creature);
    }

    /// A creature by id, whether the island seeded it or something spawned it since.
    ///
    /// Owned rather than borrowed: half of them live behind the lock. The definition is not
    /// carried with it -- look it up from [`Self::definitions`] by name, which outlives the
    /// lock, exactly as a device's is.
    pub fn creature_now(&self, id: u32) -> Option<Creature> {
        if let Some(creature) = self.creature(id) {
            return Some(creature.clone());
        }

        self.changes
            .lock()
            .expect("world lock")
            .creatures
            .iter()
            .find(|creature| creature.id == id)
            .cloned()
    }

    /// Every creature that has appeared since the island was built.
    pub fn spawned_creatures(&self) -> Vec<Creature> {
        self.changes.lock().expect("world lock").creatures.clone()
    }

    /// How much has been taken off `id`, by everybody who has hit it.
    pub fn damage_to(&self, id: u32) -> u32 {
        self.changes
            .lock()
            .expect("world lock")
            .damage
            .get(&id)
            .copied()
            .unwrap_or(0)
    }

    /// Record the total damage done to `id`.
    pub fn set_damage(&self, id: u32, damage: u32) {
        self.changes.lock().expect("world lock").damage.insert(id, damage);
    }

    /// Whether somebody has already killed `id`.
    pub fn is_dead(&self, id: u32) -> bool {
        self.changes.lock().expect("world lock").dead.contains(&id)
    }

    /// Say that `id` is dead, and answer whether this is the first time.
    ///
    /// The answer is what stops a corpse being farmed: two players swinging at the same knight
    /// both land a killing blow, and only the first is a kill.
    pub fn mark_dead(&self, id: u32) -> bool {
        self.changes.lock().expect("world lock").dead.insert(id)
    }

    // --- floor drops -----------------------------------------------------------------------

    /// Put an item on the floor, for everybody.
    pub fn drop_item(&self, drop: FloorDrop) {
        self.changes.lock().expect("world lock").drops.push(drop);
    }

    /// Everything lying on the floor.
    pub fn floor_drops(&self) -> Vec<FloorDrop> {
        self.changes.lock().expect("world lock").drops.clone()
    }

    /// Take a drop off the floor, if it is still there.
    ///
    /// **The first caller wins.** Two players standing on the same pile both fire at it, and
    /// the world is what decides that only one of them gets it.
    pub fn take_floor_drop(&self, pickup: u32) -> Option<FloorDrop> {
        let mut changes = self.changes.lock().expect("world lock");

        let at = changes.drops.iter().position(|drop| drop.pickup == pickup)?;

        Some(changes.drops.remove(at))
    }

    /// The first entity id nothing is using.
    ///
    /// Past the props the island was built with **and** past the devices restored on top of
    /// them, so the game server's allocator cannot hand out an id a restored anvil holds.
    pub fn next_entity_id(&self) -> u32 {
        let props = self.entities.iter().map(|entity| entity.id).max().unwrap_or(0);

        let changes = self.changes.lock().expect("world lock");

        let devices = changes.devices.iter().map(|device| device.id).max().unwrap_or(0);
        let creatures = changes.creatures.iter().map(|creature| creature.id).max().unwrap_or(0);

        let drops = changes
            .drops
            .iter()
            .map(|drop| drop.pickup.max(drop.stack))
            .max()
            .unwrap_or(0);

        props.max(devices).max(creatures).max(drops) + 1
    }

    /// What the terrain generator produced, before anybody touched it.
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
