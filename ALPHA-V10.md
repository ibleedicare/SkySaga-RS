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

## A dead end, recorded so it is not walked again

**The resource table is not the problem.** It looked like one: on a client that had been running
a long time, exactly one entry sat "in progress" while 43 were ready and 84 unrequested. On a
freshly launched client the histogram is `{0: 85, 2: 43}` with **nothing in progress at all**, so
the single stuck entry was a transient, not a cause.

Worse, the poll it was found through is not a symptom either. A backtrace from `FUN_005dff80`
lands in the **main game loop**:

```
0x0048b220:
  loop:  eax = [0x1440e98]                  ; app state, quit checks at +0x70/+0x72
         ecx = [edi+0x20];  call 0x5e1150   ; tick
         push 1; push 0;    call 0x5dfdd0   ; tick -> the resource poll
         ecx = [0x1440e38]; call 0x48c8c0   ; tick
         jmp loop
```

The client is ticking normally, every frame, and polling resources each frame because that is
what the loop does. The 200,000 poll hits in 40 seconds were 200,000 frames of ordinary work,
not a busy-wait. The window looks frozen because the loading screen is static, not because the
process is.

So: the client is **healthy and idle**, waiting for something, with no resource outstanding.

For reference, the poll's table layout, since the tooling reads it:

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

Entry records are 40 bytes: `+0x00` a shared vtable, `+0x04` a per-entry object carrying its own
id at `+0x10`, `+0x0a` flags, `+0x0f` the state. On a fresh client 43 entries are ready and the
rest were never requested.

## Why the client stops in LOAD_GAME_OBJECTS: the whole chain

Traced end to end, each step read out of the client rather than inferred:

```text
FUN_0080d320   requests DOWNLOAD_WORLD only when [[this+0x38]+0xA4] == 5
               that object is a ModeLevelBase; live it reads 2

FUN_004d7750   drives +0xA4 through 0..5. Only case 4 sets 5, so 2 must reach 3 first.
               case 2 (where we are) ticks vtable+0xA0 then vtable+0xBC every frame

vtable+0xBC    opens with `call vtable+0x10C; test al,al; je done` -- it does nothing
               at all unless that predicate returns true

FUN_007a19d0   that predicate returns the byte at ModeLevelBase+0x51A4; live it reads 0

FUN_007a4800   computes that byte: assume 1, then walk the list at
               [DAT_0143785c+0x12A8 .. +0x12AC] (stride 12) and clear it if any entry
               is unfinished
```

Reading that list live (`tools/level-pending-b36731.py`) gives twelve loaders:

```text
 0 w_Effects           2      6 w_EntitiesCreatures  2
 1 w_Resources         2      7 w_EntitiesDevices    2
 2 w_ScatterAssets     2      8 w_EntitiesProps      2
 3 w_Trees             1 <--  9 w_Players            2
 4 w_Tools             2     10 w_BehaviourSets      2
 5 w_CharacterParts    2     11 w_Actions            2
```

**`w_Trees` is stuck at state 1** while the other eleven reached 2. One unfinished loader clears
the readiness byte, and everything above follows from that.

Not yet established: why `w_Trees` alone fails to finish. It reached state 1, so it *started*.
Worth testing whether it correlates with what the map names, since the run above had
`terrainGenerator` at the "none" sentinel (`SKYSAGA_MAPSPEC_FILL` was off).

Driven past this point by hand the client behaves perfectly: it requests terrain, accepts all
144 chunks, and reaches POPULATE_WORLD. Nothing else in the handshake is known to be broken.

### Driving it by hand

```bash
uv run tools/loading-stage-b36731.py --object 0x10232f90 --request 7 --watch 20
```

A transition is *requested*, not assigned: `+0xD35` is the wanted stage and `+0xD36` a pending
flag that the per-frame tick consumes. Writing `+0xD34` directly only changes the text on the
loading screen, which is a good way to mislead yourself for ten minutes.

## Where to look next

The client is idle in `LOAD_GAME_OBJECTS`, sending nothing over RakNet and making no HTTP calls,
with its main loop ticking normally. So it is waiting to be told something.

The open question is **the direction of ids 9 (`FrameTimeSyncCheck`) and 10 (`ResourceCheck`)**.
`recv-map-b36731.py` resolves id 11 to a receive handler but not 9 or 10, which suggests they are
client-to-server. That is weak evidence: the tool resolves only 100 of 341 ids, so absence proves
nothing. Two attempts to settle it by looser and stricter byte patterns both failed, one with a
false positive (matching `push 0xa; push 1` in unrelated UI argument setup) and one with a false
negative (rejecting `SetConnectionTimeout`, whose `mov ecx` precedes the pushes rather than
following them). **Do not trust a pattern that has not been validated against a known handler.**

`ResourceCheck` matching the stalled stage name is suggestive enough to test empirically rather
than statically: send it and watch, the way `SetConnectionTimeout` was added. Its payload is
unknown, so start by establishing whether the client reacts at all.

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
