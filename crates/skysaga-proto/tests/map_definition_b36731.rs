//! `MapDefinition` / `MapSpec` as **build 36731** reads them.
//!
//! Layout from `documentations/packets-b36731.md` §8, recovered from the client's deserializers
//! `FUN_007e9e80` (MapDefinition) and `FUN_007e9810` (MapSpec). As with `ServerInfo` there is no
//! capture and there cannot be one — the packet is server->client only — so the expected bits are
//! built here from the specification by a builder sharing no code with `BitWriter`.
//!
//! ```text
//! MapSpec.searchable  FUN_007e9970   14 ranged fields, 2 strings, 1 plain u32, interleaved
//! MapSpec             string, 5 bits, list header, string
//! MapDefinition       string AdventureType, string MapFileName, optional UUID GroupLock,
//!                     optional u32 GameQueueCRC, 5 bits GameMode, u32 (unnamed)
//! ```
//!
//! # The widths are data, not constants
//!
//! Most fields are **GeoData table indices**, and the client computes each width at runtime from
//! the size of the table it indexes: `16 - clz16(count - 1)`. The counts below were read out of a
//! live Alpha V10 client's GeoData manager at `[0x01487bcc] + offset` (§8). They are therefore
//! **data-dependent** — if the client's GeoData changes, they change, and these tests are what
//! will catch it.
//!
//! # Which slot is which
//!
//! §8 says the field *names* (from the dumper) and the field *order* (from the deserializer) have
//! not been correlated. They now have been, by a third and independent route: **table sizes**.
//! Every count measured in the live client is exactly the matching table in build 36731's own
//! `GeoData.json` **plus one** — the client prepends a "none" sentinel — so `Adventures` (144),
//! `Regions` (24), `MapSizes` (31) and `TerrainGenerators` (10) each identify their slot uniquely.
//!
//! The three orderings agree across all seventeen slots, and two of the confirmations are
//! structural rather than numeric: slot 5 is a *string* exactly where the name list says
//! `adventureType`, slot 6 is a *plain word* exactly where it says `seed`, and slots 8-10 are three
//! consecutive fields sharing one reader function exactly where it lists three creature sets.
//!
//! Still open: slots 12 and 13 are both 3-bit fields over 5-entry tables, so size cannot separate
//! `timeOfDayPresetList` from `mapSizeCategory` — that ordering rests on the dumper's list alone.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::client_build::ClientBuild;
use skysaga_proto::packets::{MapDefinition, MapSpec};

mod bits;

use bits::{actual, Expected};

/// The GeoData table sizes measured on a live client, in wire order, with the width each gives.
///
/// `WriteIndex(count)` is `ranged(count - 1)`, so the width is `32 - clz32(count - 1)`.
const SEARCHABLE: [(u32, u32); 14] = [
    (6, 3),    //  1  FUN_007d7a70
    (25, 5),   //  2  FUN_007d8090
    (145, 8),  //  3  FUN_007d78f0
    (4, 2),    //  4  inline ranged, FUN_00ea7260(3) -- max 3, so not a table index
    (46, 6),   //  7  FUN_007d7c90
    (46, 6),   //  8  FUN_007d7d10
    (46, 6),   //  9  FUN_007d7d10
    (46, 6),   // 10  FUN_007d7d10
    (17, 5),   // 11  FUN_007d8210
    (6, 3),    // 12  FUN_007d8290
    (6, 3),    // 13  FUN_007d7f90
    (32, 5),   // 14  FUN_007d7f10
    (11, 4),   // 15  FUN_007d8190
    (4, 2),    // 17  FUN_007d7d90
];

fn fixture() -> MapDefinition {
    MapDefinition {
        spec: MapSpec {
            searchable: [0; 14],
            searchable_string_a: String::new(),
            searchable_u32: 0,
            searchable_string_b: String::new(),
            name: String::new(),
            time_of_day: 0,
            map_list: Vec::new(),
            trailing_name: String::new(),
        },
        adventure_type: String::new(),
        map_file_name: String::new(),
        group_lock: None,
        game_queue_crc: None,
        game_mode_b36731: 0,
        unnamed: 0,
        ..MapDefinition::default()
    }
}

fn encoded(map: &MapDefinition) -> String {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    map.encode(&mut writer);

    actual(&writer)
}

/// Build the expected bits straight from §8.
fn expected(map: &MapDefinition) -> String {
    let mut out = Expected::default();

    out.byte(8 + 134); // MapDefinition is id 8 in this build, not 6

    // --- MapSpec.searchable, FUN_007e9970 --------------------------------------------------
    for (index, (_, width)) in SEARCHABLE.iter().enumerate().take(4) {
        out.ranged(map.spec.searchable[index], *width);
    }

    out.string(&map.spec.searchable_string_a); // slot 5
    out.u32_be(map.spec.searchable_u32); // slot 6, a plain word

    for (index, (_, width)) in SEARCHABLE.iter().enumerate().skip(4).take(9) {
        out.ranged(map.spec.searchable[index], *width);
    }

    out.string(&map.spec.searchable_string_b); // slot 16
    out.ranged(map.spec.searchable[13], SEARCHABLE[13].1); // slot 17

    // --- the rest of MapSpec, FUN_007e9810 -------------------------------------------------
    out.string(&map.spec.name);
    out.ranged(map.spec.time_of_day, 5); // FUN_007d7970, count 28

    // The list header FUN_007e66b0 opens with FUN_007d77d0, a 4-bit tag ranged on 0xC. Zero
    // means "no elements", so nothing follows and the element loop never runs.
    out.ranged(map.spec.map_list.len() as u32, 4);

    for element in &map.spec.map_list {
        out.u32_be(*element);
    }

    out.string(&map.spec.trailing_name);

    // --- MapDefinition's own fields, FUN_007e9e80 ------------------------------------------
    out.string(&map.adventure_type);
    out.string(&map.map_file_name);
    out.optional_uuid(map.group_lock);
    out.bit(map.game_queue_crc.is_some());

    if let Some(crc) = map.game_queue_crc {
        out.u32_be(crc);
    }

    out.ranged(map.game_mode_b36731, 5);
    out.u32_be(map.unnamed);

    out.0
}

/// The whole packet, in the order §8 gives.
#[test]
fn map_definition_matches_the_2017_layout() {
    let map = fixture();

    assert_eq!(encoded(&map), expected(&map));
}

/// Every field distinct, so a transposed pair cannot pass by both being zero.
///
/// An all-zeros fixture is exactly the packet the client already rejects, and it would also
/// pass a completely mis-ordered encoder. Each slot gets a different value, clamped to its own
/// range so an out-of-range index (a different bug) cannot be what is being measured.
#[test]
fn every_slot_lands_in_its_own_field() {
    let mut map = fixture();

    for (index, (count, _)) in SEARCHABLE.iter().enumerate() {
        map.spec.searchable[index] = (index as u32 + 1).min(count - 1);
    }

    map.spec.searchable_string_a = "a".to_owned();
    map.spec.searchable_u32 = 0xdead_beef;
    map.spec.searchable_string_b = "b".to_owned();
    map.spec.name = "name".to_owned();
    map.spec.time_of_day = 7;
    map.spec.trailing_name = "trailing".to_owned();
    map.adventure_type = "Adventure".to_owned();
    map.map_file_name = "Map".to_owned();
    map.game_queue_crc = Some(0x1234_5678);
    map.game_mode_b36731 = 3;
    map.unnamed = 9;

    assert_eq!(encoded(&map), expected(&map));
}

/// The id is 8 in this build, not 6.
#[test]
fn map_definition_carries_the_2017_packet_id() {
    let mut writer = BitWriter::for_build(ClientBuild::B36731);

    fixture().encode(&mut writer);

    assert_eq!(writer.as_bytes()[0], 8 + 134);
}

/// `GameMode` is **five** bits, ranged on `0x10` — not four.
///
/// This is the one place the C# reference and the client disagree. `MapDefinition.cs` writes it
/// with `WriteIndex(0x10)`, i.e. ranged on `0x10 - 1` = 15, which is `32 - clz32(15)` = **4**
/// bits. But §8 records the client's own call as `FUN_00ea7260(0x10)` -> 27, so it reads
/// `0x20 - 27` = **5** bits. The subtraction belongs to table *indices* (`WriteIndex(count)`
/// covers `0..count-1`); `GameMode` is an inline ranged field on a declared maximum, so there is
/// nothing to subtract.
///
/// One bit short here leaves the trailing `uint32` shifted, and the C# never got past this
/// packet — consistent with a residual desync rather than a purely semantic rejection.
#[test]
fn game_mode_is_five_bits_wide() {
    let mut narrow = fixture();
    narrow.game_mode_b36731 = 0;

    let mut wide = fixture();
    wide.game_mode_b36731 = 0b1_0000; // needs the fifth bit

    let (narrow, wide) = (encoded(&narrow), encoded(&wide));

    assert_eq!(narrow.len(), wide.len(), "width must not depend on the value");

    let differing = narrow.chars().zip(wide.chars()).filter(|(a, b)| a != b).count();

    assert_eq!(differing, 1, "the fifth bit must be reachable");
}

/// A list with elements writes its count in the 4-bit tag, then the elements.
#[test]
fn the_map_list_writes_its_count_then_its_elements() {
    let mut map = fixture();
    map.spec.map_list = vec![1, 2, 3];

    assert_eq!(encoded(&map), expected(&map));
    assert_eq!(
        encoded(&map).len(),
        encoded(&fixture()).len() + 3 * 32,
        "three elements cost three words",
    );
}

/// The 2015 layout is untouched by any of this.
#[test]
fn the_retail_layout_is_unchanged() {
    let mut writer = BitWriter::new();

    MapDefinition {
        size_chunks: [4, 4, 4],
        biome: Some(0x1234_5678),
        game_mode: 1,
        ..MapDefinition::default()
    }
    .encode(&mut writer);

    let mut out = Expected::default();

    out.byte(MapDefinition::ID as u8 + 134)
        .ranged(4, 6)
        .ranged(4, 6)
        .ranged(4, 6)
        .bit(true)
        .u32_be(0x1234_5678)
        .ranged(1, 3);

    assert_eq!(actual(&writer), out.0);
}

// --- the home island, with real GeoData indices -----------------------------------------------

/// Every filled slot fits the width its table gives it.
///
/// A value that overflows its field is silently truncated by the writer and lands as a *different*
/// index — the client would resolve some other adventure, or none. `adventure` = 83 in an 8-bit
/// field is the tight one: 144 entries plus the sentinel needs all eight bits.
#[test]
fn the_home_island_indices_fit_their_fields() {
    let spec = MapSpec::home_island_b36731(1337);

    for (slot, (count, width)) in SEARCHABLE.iter().enumerate() {
        let value = spec.searchable[slot];

        assert!(
            value < *count,
            "slot {slot}: index {value} is outside its {count}-entry table",
        );
        assert!(
            value < 1 << width,
            "slot {slot}: index {value} does not fit {width} bits",
        );
    }
}

/// Index 0 means "none", so a home island must not be all zeros.
///
/// This is the whole point of the exercise: the client prepends a sentinel to every table, and a
/// map of zeros names nothing. It parses cleanly and then cannot be resolved — which is exactly
/// the state the C# server never got out of.
#[test]
fn the_home_island_is_not_all_sentinels() {
    let spec = MapSpec::home_island_b36731(1337);

    assert_eq!(spec.searchable[2], 83, "adventure must be Home_Island_Adventure");
    assert_eq!(spec.searchable[11], 4, "mapSize must be HomeIsland_6x4x6");
    assert_eq!(spec.searchable_string_b, "Home_Island_World");

    let named = spec.searchable.iter().filter(|value| **value != 0).count();

    assert!(named >= 5, "only {named} slots are filled");
}

/// The seed is the one searchable field that is not a table index.
#[test]
fn the_seed_is_carried_verbatim() {
    assert_eq!(MapSpec::home_island_b36731(0xdead_beef).searchable_u32, 0xdead_beef);
}

/// The home island still encodes to exactly the §8 layout.
#[test]
fn the_home_island_matches_the_2017_layout() {
    let mut map = fixture();
    map.spec = MapSpec::home_island_b36731(1337);

    assert_eq!(encoded(&map), expected(&map));
}
