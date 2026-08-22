# SkySaga server emulator (Rust)

A server emulator for **SkySaga: Infinite Isles**, a voxel MMO that was cancelled in 2017 and
never released. The game's servers are gone; this is a reimplementation of them, reverse
engineered from the client, so the game can be played again.

**Status: a real client logs in, creates a character, and plays.** Verified end to end against
retail build 10414 on 2026-08-21: account login, character creation with appearance and name,
terrain, entities, and world entry, all served by this code with no C# server running.

It is a rewrite of an earlier C# emulator, which is kept as a reference implementation and as
a test oracle (see [Tests](#tests)).

## Before you clone

**This repository does not build on its own.** It is one directory inside a larger working
tree, and it needs two things from outside itself:

| Needed | Why | Where it comes from |
|---|---|---|
| `Entities.json` | every entity's components and sync indices; the world cannot be built without it | the C# emulator's `Data/` directory; point `SKYSAGA_DATA_DIR` at a copy |
| `libRakNet.so` | the game protocol is RakNet/SLikeNet; the client speaks nothing else | built from SLikeNet source: `nix build .#raknet` in the parent tree, or set `SKYSAGA_RAKNET_LIB` |

Neither is redistributable here. `Entities.json` is the game's own data, and the RakNet build
is a native library, not source. `cargo build` fails at the `raknet-sys` build script if it
cannot find the library, and the server exits at startup if it cannot find `Entities.json`.

You also need the game client itself, which is not public. This is emulator source, not a way
to obtain the game.

## Layout

```
crates/
  skysaga-core/     name hashing, bit maths, fixed-width strings          (pure)
  skysaga-state/    accounts, sessions, characters, photos                (pure)
  skysaga-store/    persistence: the Store trait, and SQLite
  skysaga-proto/    packet wire formats and the RakNet BitStream codec    (pure)
  skysaga-world/    entity definitions, components, terrain generation    (pure)
  skysaga-auth/     Smilegate login                          TCP  :10106
  skysaga-web/      account / characters / conductor / social / photos
                                                             HTTP :5164
  skysaga-game/     the RakNet game server and session state machine
                                                             UDP  :42069
  skysaga-server/   one binary running all of them over one shared state
  raknet/           safe wrapper over…
  raknet-sys/       …the SLikeNet C API
```

The four `(pure)` crates do no I/O at all, which is why most of the test suite needs neither a
socket nor a running server. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design rules and
the reasoning behind them.

## Running it

```bash
cargo run --release -p skysaga-server
```

That one process serves everything the client needs. Then launch the client pointed at
`127.0.0.1`; any non-empty account name is accepted by default.

There is also a `skysaga-game` binary that runs *only* the game server, for working on the
world without the web stack. Note that the world-shaping variables below are read by that
binary alone. `skysaga-server` builds its world from the defaults and ignores them.

### Configuration

Read by `skysaga-server`:

| Variable | Default | |
|---|---|---|
| `SKYSAGA_ACCOUNTS` | *(unset)* | `user:pass,other:pass` to restrict logins; unset accepts any non-empty name |
| `SKYSAGA_PUBLIC_IP` | `127.0.0.1` | the address handed to the client; set this when the client is not on this host |
| `SKYSAGA_WEB_PORT` | `5164` | |
| `SKYSAGA_AUTH_PORT` | `10106` | |
| `SKYSAGA_GAME_PORT` | `42069` | |
| `SKYSAGA_DATA_DIR` | *(the C# tree)* | directory holding `Entities.json` |
| `SKYSAGA_DATABASE_URL` | `sqlite://skysaga.db` | where state is persisted; set it empty to keep everything in memory |
| `SKYSAGA_RAKNET_LIB` | *(`../.raknet/lib`)* | directory holding `libRakNet.so`; read at build time |
| `RUST_LOG` | `info` | e.g. `skysaga_web=debug` to log every request body |

Read by `skysaga-game` only: `SKYSAGA_ADVENTURE`, `SKYSAGA_BIOME`, `SKYSAGA_WORLD_TYPE`,
`SKYSAGA_WORLD_SEED`, `SKYSAGA_WORLD_CHUNKS`, `SKYSAGA_SPAWN_CLEARANCE`,
`SKYSAGA_TIME_OF_DAY`, `SKYSAGA_PLAYER_NAME`.

### Persistence

Accounts, characters and photos are stored in SQLite and survive a restart. The database is
created on first run; there is nothing to set up.

State is held in memory while the server runs and written down as it changes, so nothing is
on a request path. That means a write is durable a moment after the change rather than at the
instant of it, and a crash can lose the last few seconds. Ordering is preserved, so a delete
never loses a race with the create before it.

`skysaga-store` is built around a `Store` trait, with SQLite implemented today. PostgreSQL is
adding one file: implement the trait, and the existing tests are the specification, because
they are written against the trait rather than against SQLite.

### Starting character creation again

A finished character now outlives the server, not just the client. Once it has a home biome,
`characters/list` reports a *complete* character and the client skips its creator entirely,
which looks exactly like a broken creator when nothing is wrong. To go through it again:

```bash
curl -X POST 'http://127.0.0.1:5164/debug/reset-character'
```

The account stays signed in; only the character is discarded, in memory and on disk.

## Tests

```bash
cargo test --workspace          # 469 tests, no network, nothing to prepare
```

The tests are the point of the rewrite, so a word on what they actually check.

**Some of them drive a headless client over a real socket.** `skysaga-probe` speaks the
protocol without rendering anything, so "does the server actually answer that packet" is a
test that runs in a second rather than a Wine client and a human looking at a screen. The
`parity_*` files use it: they start this server in-process, play a scenario, and assert what
came back. That is the layer where the inventory packets were failing — they decoded fine and
were then dropped, which from a player's side is a UI that freezes rather than an error.

**Those same scenarios can be replayed against the running C# server.** Start it beside this
one and point the tests at it:

```bash
./scripts/run-oracle.sh                                  # C# on :43069, admin panel on :6175
SKYSAGA_ORACLE_GAME=127.0.0.1:43069 \
  SKYSAGA_ORACLE_ADMIN=http://127.0.0.1:6175 \
  cargo test -p skysaga-probe
```

Without those variables the oracle tests **skip** rather than fail, so `cargo test --workspace`
stays runnable with nothing prepared. Two behavioural differences were found this way rather
than by reading the C#, and both are now asserted on each side: it echoes a mover its own
position, and its idea of which way a player faces is always approximately zero.

**The C# server is the oracle, not this code's own opinion.** The fixtures under
`crates/*/tests/` were captured by running the real C# servers and recording what they put on
the wire, including a full RakNet handshake replayed byte for byte. A test passes when this
server's output matches *the C#'s*, not when it matches what the author believed the format
to be.

That distinction has caught real bugs, including one that would otherwise have been invisible:
a golden-vector generator asked the C# for the wrong operation, so the C# obligingly
reproduced the mistake and every test passed. It was caught by decoding a capture from the
live client, where big-endian ids resolved to real entity names and little-endian ones
resolved to nothing.

**Where this server deliberately differs from the C#, a test asserts the C#'s behaviour too**,
so the divergence stays deliberate. Two examples: the C# places a tree at the origin because
it looks the position up on a component the entity does not have, and it never replicates a
character's appearance at all because the component class was never written and its reflective
loader skips missing classes silently.

**HTTP tests do not bind a port.** Requests go through the router with
`tower::ServiceExt::oneshot`, so the suite runs in about a second and never races.

## Improvements over the C#

Defects found while reading the original, fixed here rather than reproduced:

- **The auth server assumed one `read()` returned the whole packet.** TCP may split it
  anywhere; there is a test that sends the header, pauses, then dribbles the body two bytes at
  a time.
- **The auth server handled one connection at a time**, so a client that connected and stalled
  blocked everyone else from signing in.
- **`Session.AccountName` and `_characterUUID` were process-wide statics**, so the emulator
  served exactly one player and two clients corrupted each other's state.
- **Three processes sharing no state.** One process over one `Arc<AppState>` here. As
  separate processes, a client could finish creating a character and have the web layer still
  report it as unfinished, looping it back into the creator.
- **One packet per 30 ms tick**, which capped the whole server at ~33 packets a second and was
  the documented cause of its interaction lag. This drains the queue.
- **Character appearance never replicated.** `Player` sync index 19 was silently absent, so
  every character rendered with the client's built-in defaults no matter what was chosen in
  the creator.

## Known gaps

- **The friends graph is not interactive.** Character search finds a character and the
  response *shapes* are all implemented, being the part that is easy to get wrong, but adding,
  accepting and blocking are acknowledged rather than recorded.
- **No chat.** The client expects an IRC server on :4444 and retries forever without one; it
  is noisy in the client log but does not block play.
- **No HTTPS.** The 2017 builds (Alpha V10 b36731) need it; retail 10414, which this was
  verified against, is plain HTTP.
- **Buying from the trading post is not implemented.** Browsing works: the catalogue and the
  search both answer. A purchase is a *teleport* to the seller's home island rather than an
  item transfer, so it belongs to the world-transfer work rather than to the trading routes.
- **Only a chest is open-able, and each connection has its own view of it.** The mailbox and
  the crafting stations need their own handlers; and two players looking into one chest see
  two different sets of contents, because the container store is per session rather than
  shared.

## Contributing

The layout is chosen so that adding something is adding a file:

- **An HTTP endpoint** → one `async fn` plus one `.route()` line in that module's `router()`.
- **A packet** → one struct with an `ID` and an `encode`/`decode` in
  `skysaga-proto/src/packets/`, one variant on `ClientPacket`, one arm in the dispatch match.
- **A component** → one struct, one `Component` variant, one arm each in `sync` and `name`.
  Both matches are exhaustive, so a missing arm is a compile error rather than a parameter
  that quietly stops replicating. That is exactly how the C# lost the appearance component.

Write the test first. Where behaviour has to match the client, capture what the client or the
C# actually does rather than asserting what you think it does; two of the longest debugging
sessions in this port's history came from a decoder being *stricter* than the C# about fields
neither of them reads.
