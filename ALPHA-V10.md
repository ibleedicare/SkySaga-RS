# Serving Alpha V10 (build b36731)

What is known about getting the 2017 client into a world, how it was established, and where it
currently stops. Companion to `documentations/packets-b36731.md`, which this corrects in two
places.

## Running it

The 2017 client and the 10414 client can be online at the same time, on separate ports:

```bash
# web + auth, plain http (TLS is terminated in front, see README)
SKYSAGA_WEB_PORT=5166 SKYSAGA_AUTH_PORT=10107 SKYSAGA_GAME_PORT=42070 \
    ./target/release/skysaga-server

socat OPENSSL-LISTEN:5165,reuseaddr,fork,cert=skysaga-dev-cert.pem,key=skysaga-dev-key.pem,\
verify=0,cipher=ALL:@SECLEVEL=0,openssl-min-proto-version=TLS1 TCP:127.0.0.1:5166

SKYSAGA_CLIENT_BUILD=36731 SKYSAGA_GAME_PORT=42070 ./target/release/skysaga-game

nix develop ./nix --command ./scripts/run-alpha10-patched.sh
```

Give the web server a distinct binary name if another SkySaga server is running: a
`pkill -f skysaga-server` from another session matches this one too.

## How far it gets

```
login (https)  ->  character  ->  game server address  ->  RakNet connect
ServerInfo + MapDefinition + SetConnectionTimeout  ->  client resolves the world
client stays connected, reaches LOAD_GAME_OBJECTS, and stops there
```

The client used to be dropped ten seconds after `MapDefinition`. It no longer is, which is
further than the C# emulator ever reached.

## The two corrections to packets-b36731.md

### Index 0 means "nothing"

§9 concluded the client "cannot resolve a world" from a map of zeros and treated that as
semantic. The mechanism is arithmetic. **Every GeoData table in the client holds one more entry
than `GeoData.json`**, because it prepends a sentinel:

| table | JSON | client |
|---|---|---|
| Adventures | 144 | 145 |
| Regions | 24 | 25 |
| MapSizes | 31 | 32 |
| TerrainGenerators | 10 | 11 |

So a wire index is `position + 1`, and index 0 is "none". An all-zero `MapSpec` is not an empty
map; it is a map that names nothing.

### The MapSpec slots are correlated

§8 says the field names (from the dumper) and the field order (from the deserializer) "have NOT
been correlated". Table sizes correlate them, uniquely for Adventures, Regions, MapSizes and
TerrainGenerators, and the three orderings agree across all seventeen slots:

| # | width | table (client count) | name |
|---|---|---|---|
| 1 | 3 | Biomes (6) | `biome` |
| 2 | 5 | Regions (25) | `region` |
| 3 | 8 | Adventures (145) | `adventure` |
| 4 | 2 | inline, max 3 | `difficulty` |
| 5 | string | | `adventureType` |
| 6 | u32 | | `seed` |
| 7 | 6 | BiomePalettes (46) | palette |
| 8 | 6 | CreatureSets (46) | `featureCreatureSet` |
| 9 | 6 | CreatureSets | `terrainCreatureSet` |
| 10 | 6 | CreatureSets | `caveCreatureSet` |
| 11 | 5 | TimeOfDayPresets (17) | `timeOfDayPreset` |
| 12 | 3 | TimeOfDayPresetLists (6) | `timeOfDayPresetList` |
| 13 | 3 | MapSizeCategories (6) | `mapSizeCategory` |
| 14 | 5 | MapSizes (32) | `mapSize` |
| 15 | 4 | TerrainGenerators (11) | `terrainGenerator` |
| 16 | string | | `featureName` |
| 17 | 2 | Events (4) | `activeEvent` |
| tail | 5 | AdventureCosts (28) | `cost` |

Two of the confirmations are structural rather than numeric: slot 5 is a string exactly where
the name list says `adventureType`, slot 6 a plain word where it says `seed`, and slots 8 to 10
are three consecutive fields sharing one reader function exactly where it lists three creature
sets.

**Still open:** slots 12 and 13 are both 3-bit over 5-entry tables, so size cannot separate
`timeOfDayPresetList` from `mapSizeCategory`. That ordering rests on the dumper's list alone.

### GameMode is five bits

The C# writes it with `WriteIndex(0x10)`, which is ranged on `0x10 - 1` and gives 4 bits. The
client's own call is `FUN_00ea7260(0x10)` -> 27, so it reads `0x20 - 27` = **5**. The `- 1`
belongs to table indices; `GameMode` is an inline ranged field on a declared maximum.

## Where it stops, and what has been ruled out

The client sits in its `LOAD_GAME_OBJECTS` stage: **busy, not blocked** (`State: R`), sending
nothing over RakNet and making no HTTP requests.

Its loading stage is a byte at `object + 0xD34`, indexing the name array at `0x013fc1e8`:

```
0 LOAD_GEO  1 STARTUP  2 CONFIG_GRAPHICS  3 WAIT_FOR_SERVER  4 CONNECT_TO_SERVER
5 LOAD_GAME_OBJECTS  6 LOAD_EDITOR  7 DOWNLOAD_WORLD  8 POPULATE_WORLD
9 TELEPORTING  10 EDITOR  11 VIEWER  12 LEAVING_WORLD  13 READY_TO_PLAY
14 CHARACTER_CREATION
```

Get the object address by breakpointing `0x008f7a7a` and reading `eax`
(`tools/loading-stage-b36731.py`).

Tested and eliminated:

| hypothesis | result |
|---|---|
| the server should push terrain unprompted | client **disconnects**; `DOWNLOAD_WORLD` is a later stage than the one it is stuck in |
| `featureName` names something unresolvable | no change; the client tolerates a feature it cannot find |
| the "none" slots need real values | no change with region, palette, creature sets and terrain generator all filled |
| the client was never told to advance | forcing the stage byte to 7 changes the **screen text only**; no packet, no progress |

That last one is worth remembering: the stage byte is display state. Writing it makes the
loading screen claim a stage the client is not in.

## The current lead

The main thread spins in `FUN_005dff80`, a resource-status lookup keyed by a 16-bit id:

```
mov  eax, [edi + 0x30]              ; table base
lea  ecx, [ebx + ebx*4]             ; id * 5
cmp  byte [eax + ecx*8 + 0xf], 2    ; entry = table + id*40, state at +0xF
```

Reading that table out of the live client (`tools/resource-states-b36731.py`) gives a sharp
signal:

```
128 entries; state histogram: {0: 84, 1: 1, 2: 43}
```

43 ready, 84 never requested, and **exactly one entry in progress**: id 33, which does not move
in 30 seconds of watching. Its record differs from ready entries at `+0x0a` (11, where ready
entries have 2) and `+0x14` (1, where they have 2 or 5). Its `+0x04` object carries its own id
at `+0x10`, and `+0x30` points at a BlitzTech resource blob (`BLTZ`, `iHDR`, `iITM`, `iMNI`,
`iCHN`, `iTGT`, `iKEY` chunk tags).

**Not yet established:** whether entry 33 being stuck is the cause of the stall or a normal
resident state for a streaming resource. Identify what it is before building on it.

## Tools

| | |
|---|---|
| `tools/gen-packet-map-b36731.py` | regenerates `client_build.rs` from the doc's id table |
| `tools/loading-stage-b36731.py` | read, watch or force the client's loading stage |
| `tools/resource-states-b36731.py` | dump the resource table, find what never loads |
| `tools/dump-resource-entry-b36731.py` | whole entries, for comparing stuck against ready |
| `tools/name-geodata-tables-b36731.py` | GeoData table counts from a live client |
| `scripts/attach-when-client-starts.sh` | arm the debugger before launching the game |

All the memory readers need `kernel.yama.ptrace_scope=0` and are read-only except
`loading-stage-b36731.py --set`.
