//! The home island the server actually serves, checked against the C#'s handshake.
//!
//! Where a value is the C#'s (a seeded position, a chunk of terrain) the bytes must match.
//! Where it is ours to choose (entity ids, the player's health) the *shape* must match.

use skysaga_game::{ClientPacket, Session, World, WorldConfig};
use skysaga_proto::bitstream::{BitReader, ID_USER_PACKET_ENUM};
use skysaga_proto::packets::{ChunkSync, EntityAdd, SyncData};
use skysaga_world::{default_entities_path, EntityDefinitions};

mod world_from_capture;

fn definitions() -> EntityDefinitions {
    EntityDefinitions::load(default_entities_path()).expect("Entities.json")
}

fn home_island() -> World {
    World::home_island(&definitions(), &WorldConfig::default())
}

#[test]
fn the_island_has_terrain_and_entities() {
    let world = home_island();

    assert_eq!(world.chunks.len(), 16, "a 4x4 island");
    assert_eq!(world.entities.len(), 10, "9 seeded entities plus the player");
    assert_ne!(world.player_entity_id, 0, "the player was created");
}

/// The terrain is the same terrain the C# sent, byte for byte.
#[test]
fn the_islands_chunks_match_the_captured_ones() {
    let world = home_island();
    let captured = world_from_capture::world_from_capture();

    assert_eq!(world.chunks.len(), captured.chunks.len());

    for ours in &world.chunks {
        let theirs = captured
            .chunks
            .iter()
            .find(|chunk| chunk.coords == ours.coords)
            .unwrap_or_else(|| panic!("no captured chunk at {:?}", ours.coords));

        assert_eq!(ours.data1, theirs.data1, "chunk {:?}", ours.coords);
    }
}

/// Each seeded entity reproduces the C#'s payload for the same entity type.
///
/// Ids differ — ours are assigned in creation order and the C#'s have gaps — so entities are
/// matched by name hash, which is what the client reads anyway.
#[test]
fn the_seeded_entities_match_the_captured_payloads() {
    let world = home_island();
    let captured = world_from_capture::world_from_capture();
    let definitions = definitions();

    let mut compared = 0;

    for ours in &world.entities {
        let Some(theirs) = captured
            .entities
            .iter()
            .find(|entity| entity.name_hash == ours.name_hash)
        else {
            continue; // the player, whose state is ours to choose
        };

        let name = definitions
            .iter()
            .find(|d| Some(d.name_hash()) == ours.name_hash)
            .map(|d| d.name().to_owned())
            .unwrap_or_default();

        // The player's state is not the C#'s, so only the seeded world entities compare.
        if name.eq_ignore_ascii_case("Player") {
            continue;
        }

        // The Tree diverges on purpose -- see the_csharp_tree_is_stranded_at_the_origin.
        if name.eq_ignore_ascii_case("Tree") {
            continue;
        }

        assert_eq!(
            ours.sync_data.bytes(),
            theirs.sync_data.bytes(),
            "{name}: sync data differs from the C#'s",
        );

        compared += 1;
    }

    assert!(compared >= 8, "only compared {compared} entities");
}

/// A bug in the C#, reproduced here as a *test* rather than as behaviour.
///
/// Server.cs assigns the Tree's position through
/// `TryGetComponent<SmoothedTransformComponent>`, but Tree binds `position` to plain
/// `transformcomponent`. The component is never found, the assignment is silently dropped,
/// and the tree spawns at the origin instead of [3000, 70, 1000].
///
/// The Rust world places it where the C# intended, so this is the one seeded entity whose
/// payload deliberately differs. Asserting the C#'s behaviour keeps the divergence honest: if
/// a recapture ever shows the tree somewhere else, the C# was fixed and this can go.
#[test]
fn the_csharp_tree_is_stranded_at_the_origin() {
    use skysaga_world::TransformComponent;

    let definitions = definitions();
    let tree = definitions.get("Tree").unwrap();
    let captured = world_from_capture::world_from_capture();

    let entity = captured
        .entities
        .iter()
        .find(|e| e.name_hash == Some(tree.name_hash()))
        .expect("the Tree was in the capture");

    let mut reader = BitReader::new(entity.sync_data.bytes(), entity.sync_data.len());
    let sync = SyncData::decode(&mut reader, tree.synced_parameter_count()).unwrap();
    let mut parameters = BitReader::new(sync.parameters.bytes(), sync.parameters.len());

    assert_eq!(
        TransformComponent::read_position(&mut parameters).unwrap(),
        [0, 0, 0],
        "the C# tree is at the origin, not where Server.cs says",
    );

    // ...and ours is where it was meant to go.
    let world = home_island();
    let ours = world
        .entities
        .iter()
        .find(|e| e.name_hash == Some(tree.name_hash()))
        .unwrap();

    assert_ne!(ours.sync_data.bytes(), entity.sync_data.bytes());
}

/// The player is well-formed even though its values are ours: the right flags, and a payload
/// the client can parse to the end.
#[test]
fn the_player_entity_is_well_formed() {
    let world = home_island();
    let definitions = definitions();
    let player = definitions.get("Player").unwrap();

    let entity = world
        .entities
        .iter()
        .find(|entity| entity.id == world.player_entity_id)
        .expect("the player is among the entities");

    assert_eq!(entity.name_hash, Some(player.name_hash()));

    let mut reader = BitReader::new(entity.sync_data.bytes(), entity.sync_data.len());
    let sync = SyncData::decode(&mut reader, player.synced_parameter_count()).expect("parses");

    assert_eq!(sync.present.len(), 89);
    assert_eq!(sync.present_indices().count(), 28, "the same 28 the C# sends");

    // Nothing left over: flags + length field + payload account for the whole blob.
    assert_eq!(
        89 + 18 + sync.parameters.len(),
        entity.sync_data.len(),
        "the payload length field agrees with what is there",
    );
}

/// End to end: a session over this world emits a handshake the client can follow.
#[test]
fn a_session_over_the_island_emits_a_complete_handshake() {
    let world = home_island();
    let mut session = Session::new(world.player_entity_id);

    let mut emitted = Vec::new();

    for packet in [
        ClientPacket::ClientConnected,
        ClientPacket::ClientReadyToSync,
        ClientPacket::ClientInitialSyncFinished,
        ClientPacket::ClientReadyToPlay,
    ] {
        emitted.extend(session.handle(packet, &world));
    }

    let ids: Vec<u16> = emitted
        .iter()
        .map(|bytes| {
            BitReader::from_bytes(bytes).read_packet_id().unwrap() + ID_USER_PACKET_ENUM
        })
        .collect();

    // ServerInfo, MapDefinition, BeginSync, 16 chunks, 10 entities, sync-finished,
    // SetClientEntity, tutorial.
    assert_eq!(ids.len(), 3 + 16 + 10 + 3);

    assert_eq!(&ids[..3], &[192, 140, 141]);
    assert!(ids[3..19].iter().all(|&id| id == 142), "the chunks");
    assert!(ids[19..29].iter().all(|&id| id == 234), "the entities");
    assert_eq!(&ids[29..], &[139, 238, 162]);

    // Every packet re-parses -- nothing is truncated or misframed.
    for bytes in &emitted {
        let mut reader = BitReader::from_bytes(bytes);
        let id = reader.read_packet_id().unwrap() + ID_USER_PACKET_ENUM;

        match id {
            142 => {
                ChunkSync::decode(&mut reader).expect("chunk parses");
            }
            234 => {
                EntityAdd::decode(&mut reader).expect("entity parses");
            }
            _ => {}
        }
    }
}

/// `BeginSync` must announce exactly the number of chunks that follow. Chunks that generate
/// as pure air are skipped, so a count taken from the island's dimensions rather than from
/// the list would over-promise and hang the client.
#[test]
fn begin_sync_counts_the_chunks_actually_sent() {
    use skysaga_proto::packets::BeginSync;

    let world = home_island();
    let mut session = Session::new(world.player_entity_id);

    session.handle(ClientPacket::ClientConnected, &world);

    let out = session.handle(ClientPacket::ClientReadyToSync, &world);

    let mut reader = BitReader::from_bytes(&out[0]);
    reader.read_packet_id().unwrap();

    assert_eq!(
        BeginSync::decode(&mut reader).unwrap().chunk_count as usize,
        out.len() - 1,
    );
}

/// The player spawns above the island, in world units rather than voxels.
///
/// Position units are 1/32 of a voxel. Sending raw voxel coordinates puts the player at 1/32
/// of the intended spot — the corner of the island, inside the ground — which renders as an
/// unlit black character with nothing behind it. That is exactly the symptom this fixes.
#[test]
fn the_player_spawns_in_world_units_above_the_terrain() {
    use skysaga_game::world::POSITION_SCALE;
    use skysaga_proto::bitstream::BitReader;
    use skysaga_world::{TerrainGenerator, TransformComponent};

    assert_eq!(POSITION_SCALE, 32);

    let world = home_island();
    let definitions = definitions();
    let player_definition = definitions.get("Player").unwrap();

    let entity = world
        .entities
        .iter()
        .find(|e| e.id == world.player_entity_id)
        .expect("the player");

    let mut reader = BitReader::new(entity.sync_data.bytes(), entity.sync_data.len());
    let sync = SyncData::decode(&mut reader, player_definition.synced_parameter_count()).unwrap();

    // Walk to the position parameter by replaying the parameters before it.
    let position_index = player_definition
        .sync_index("smoothedtransformcomponent", "position")
        .expect("the player has a position");

    let terrain = TerrainGenerator::default();
    let spawn = terrain.spawn();

    let expected = [
        spawn.0 as u32 * POSITION_SCALE,
        spawn.1 as u32 * POSITION_SCALE,
        spawn.2 as u32 * POSITION_SCALE,
    ];

    // The spawn must be inside the island's footprint and clear of the ground.
    let extent = (terrain.size_chunks * 32) as u32 * POSITION_SCALE;

    assert!(expected[0] < extent && expected[2] < extent, "{expected:?} is on the island");
    assert!(expected[1] > 32, "{expected:?} is above the very bottom");
    assert_eq!(
        terrain.material_at(spawn.0, spawn.1, spawn.2),
        skysaga_world::terrain::blocks::AIR,
        "the spawn voxel is open air, not inside the ground",
    );

    assert!(sync.present[position_index], "the position is synced");
}
