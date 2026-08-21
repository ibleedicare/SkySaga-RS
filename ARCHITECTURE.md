# SkySaga server emulator — Rust architecture

A rewrite of the C# emulator (`server/Servers/`, ~33k LOC across four projects) in Rust.

This document is the *design contract*. The C# server is the behavioural oracle: when this
document and the C# disagree about bytes on the wire, the C# wins and this document is wrong.
When they disagree about *structure*, this document wins — the point of the rewrite is that
the structure is contributable.

---

## What is wrong with the current shape

Not a criticism of the original — it grew by reverse-engineering, which is the only way it
could have been written. But it now has four properties that make contribution hard:

| Problem | Where | Consequence |
|---|---|---|
| Three processes, no shared state | `SkySaga.Web` / `SmilegateAuth` / `SkySaga.Game` | The web server knows the account name; the game server cannot see it. State is duplicated or guessed. |
| Global mutable statics | `Web/Session.cs`, `PersistentRecordEndpoints._characterUUID` | Exactly one player, ever. Two clients corrupt each other. |
| Networking fused to game logic | `Connection.cs` — 2827 lines | Nothing about the world can be tested without a RakNet socket and a real client. |
| Reflection-driven components | `EntityManager.cs` (`Activator.CreateInstance`) | Rust has no reflection, so this cannot be transliterated; and the C# form hides which components exist. |
| Hardcoded world seeding | `Server.cs` ctor, `Connection::InitialChunkSync` (`Data1[200+1024+1024+1] = 13`) | Adding a map means editing magic array indices. |

---

## Shape of the port

```
rust-server-port/
├── Cargo.toml                workspace
├── ARCHITECTURE.md           this file
└── crates/
    ├── skysaga-core/         pure values: name hashing, bit maths, fixed-width strings, ids
    ├── skysaga-auth/         Smilegate login   (TCP  :10106)
    ├── skysaga-web/          account / conductor / social API   (HTTP :5164, HTTPS :5165)
    ├── skysaga-state/        the shared world-independent state: accounts, characters, sessions
    ├── skysaga-proto/        RakNet packet wire formats — encode/decode, no I/O   [stage 2]
    ├── skysaga-world/        entities, components, chunks — pure simulation, no I/O [stage 2]
    ├── raknet/               safe wrapper over the native shim                      [stage 2]
    ├── skysaga-game/         game server: joins net ↔ world                         [stage 2]
    └── skysaga-server/       one binary that runs auth + web + game together
```

Stage 1 (this milestone: *a client can log in*) needs only `core`, `state`, `auth`, `web`,
`server`. The stage-2 crates are listed so the layout does not have to be re-cut later.

### Rule 1 — I/O lives at the edges

`skysaga-core`, `skysaga-state`, `skysaga-proto` and `skysaga-world` have **no** sockets, no
files, no clock, no `tokio`. Everything they do is a function from values to values, which
means everything they do is unit-testable without a client, a network or a fixture directory.

`auth`, `web` and `game` are thin: they move bytes in, call a pure function, move bytes out.

This is the single change that makes the project testable. `Connection.cs` cannot be tested
because there is no seam between "read a packet" and "decide what happens"; here there always
is one.

### Rule 2 — one process, one shared state

The C#'s three processes are an accident of how it was built, and it costs correctness: the
game server has to re-derive the account name that the web server already knew.

`skysaga-server` is one binary running three `tokio` tasks over one `Arc<AppState>`. The
individual binaries (`skysaga-auth`, `skysaga-web`) stay runnable on their own so a
contributor can work on one piece, and so the Rust web server can be swapped in while the C#
game server still runs — which is how the port gets validated incrementally.

### Rule 3 — no global mutable state, ever

`AppState` holds `accounts: RwLock<HashMap<AccountId, Account>>` and friends. Multiple
players work by construction rather than by later effort. Nothing is `static mut`, nothing is
a `lazy_static` singleton, nothing is "single-slot because the emulator serves one player".

### Rule 4 — adding a thing is adding a file

The contribution test for every subsystem:

- **A new HTTP endpoint** → one `async fn` + one `.route()` line in that module's `router()`.
- **A new packet** → one file in `skysaga-proto/src/packets/`, one variant on `PacketId`, one
  arm in the dispatch `match`. The compiler tells you if you miss the arm.
- **A new component** → one variant on `enum Component` + its `sync()` arm. Exhaustive
  `match` means a missing case is a compile error, not a silent no-op.

No reflection, no registries keyed by string, no `Activator.CreateInstance`. If it can be an
exhaustive `match`, it is one.

### Rule 5 — the wire format is written down, not inferred

C#'s `[StructLayout(Pack = 1)]` marshalling means the layout of `LoginReply` exists only as a
consequence of field declaration order. Here every wire struct has an explicit
`const SIZE: usize`, an explicit field-offset table in a doc comment, and a test asserting a
known-good byte sequence. Layout is stated, then verified.

---

## Testing strategy (TDD, in the order tests are written)

1. **Unit tests, pure crates.** Written *before* the implementation. Known-good vectors come
   from the C# — CRC32 hashes, packet sizes, byte-for-byte encodings.
2. **Golden-byte tests.** `LoginRequest`/`LoginReply` and every RakNet packet get a
   hex fixture checked in under `tests/golden/`. A wrong integer width fails a test in
   milliseconds instead of appearing as "client stuck at Ready to Play".
3. **HTTP contract tests.** Each endpoint is exercised in-process through
   `tower::ServiceExt::oneshot` — no port binding, no sleep, fast. Assertions are on the exact
   JSON keys, because the client reads them case-sensitively (`RESERVED_NAME` must not become
   `reserveD_NAME` — a real bug the C# had to work around).
4. **The client.** The final gate. It is unforgiving and specific: it hangs at a *named*
   loading stage, and the name says which packet is wrong.

Rule: no implementation code is written for behaviour that does not yet have a failing test,
except for glue that has no behaviour of its own (a `main` that binds a port).

---

## Improvements deliberately carried in

Real defects found in the C# while reading it, fixed as part of the port rather than
faithfully reproduced:

- **Auth server assumes one `read()` returns the whole packet** (`SmilegateAuth/Program.cs:22`).
  TCP may split it. `read_exact` on the 5-byte header, then on the body.
- **Auth server is single-connection, serial.** One task per connection.
- **`Session` and `_characterUUID` are process-wide singletons.** Keyed by account here.
- **The game server drains one packet per 30 ms tick** (`Server.cs`), a 33 packet/s ceiling
  for the entire server. Drain until empty. *(stage 2)*
- **`ToFrozenSet()` rebuilt every tick** for structures that are build-once. *(stage 2)*

---

## Stage plan

| Stage | Deliverable | Done when |
|---|---|---|
| **1** | `core` + `state` + `auth` + `web` + `server` | The client logs in and reaches character select against the Rust servers, with the C# web and auth servers stopped |
| 2 | `raknet` + `proto` | Handshake completes; golden packet sizes match the C# table |
| 3 | `world` + `game` | The client enters the world; all C# stopped |
| 4 | persistence, chat, beyond parity | — |

Ports, unchanged from the C# so the existing launch scripts keep working:

| Service | Port | |
|---|---|---|
| web | 5164/tcp | http — build 10414 |
| web | 5165/tcp | https — build 36731 (`SKYSAGA_WEB_HTTPS=1`) |
| auth | 10106/tcp | Smilegate login |
| game | 42069/udp | RakNet |

---

## Addendum — `skysaga-proto` and the profile (stage 2, partial)

`skysaga-proto` landed ahead of the RakNet transport, because the wire formats are pure
functions and can be tested without one. It holds:

- `bitstream` — a pure-Rust reimplementation of the subset of RakNet's `BitStream` the game
  uses, verified byte-for-byte against the real `libRakNet.so`.
- `customisation` — `CustomisationData`: gender, tribe, skin/eye/clothing materials and the
  hair attachment.
- `packets` — the character-creation packets: `SaveCharacterName` (108),
  `CharcterCreationResponse` (109), `CreateHomeworld` (110),
  `SetCharacterCustomisationData` (37).

**`skysaga-state` depends on `skysaga-proto`** for `CustomisationData`. That is deliberate:
a character's appearance *is* a protocol value, and defining a parallel type in `state` would
mean converting between two identical structs at every boundary. Both crates are pure, so the
dependency costs nothing in testability.

### Generating the oracle

The vectors in `crates/skysaga-proto/tests/fixtures/bitstream.tsv` come from
`tools/bitstream-golden`, a small C# program that drives the real RakNet `BitStream` and
prints `label<TAB>bits<TAB>hex`. Regenerate it when adding a packet:

```bash
dotnet run -c Release --project tools/bitstream-golden \
  > crates/skysaga-proto/tests/fixtures/bitstream.tsv
```

It needs the C# tree next door and `libRakNet.so`; it is a development tool, not part of the
server. Writing the packet's shape into that generator *first*, then making Rust reproduce its
output, is the workflow — it is what stops the tests from merely agreeing with the Rust.

### What the profile still needs

The packets are implemented and tested; nothing dispatches them yet, because there is no
RakNet transport. When one lands, wiring is:

| packet | handler calls |
|---|---|
| `SaveCharacterName` (108) | `AppState::set_character_name`, then reply `CharacterSaved` |
| `CreateHomeworld` (110) | `AppState::set_home_biome`, then reply `HomeworldCreated` |
| `SetCharacterCustomisationData` (37) | `AppState::set_appearance` |

All three already exist and are tested. The reply is not optional: without
`CharacterSaved` the client's creator waits forever.

---

## Addendum — the RakNet transport

Two crates, and they are the only place `unsafe` appears in the port:

- `raknet-sys` — 19 `extern "C"` declarations, no code.
- `raknet` — a safe wrapper: `Peer`, `Packet`, `Guid`, with `Drop` on both handles.

### No C++ shim, after all

The roadmap called for a hand-written C++ shim over SLikeNet, on the grounds that the SWIG
wrapper carries the LP64 `long` bug (`Write< long >` is 32 bits on MSVC, 64 on Linux). Two
things retired that plan:

1. The flake already narrows `long` to `int32_t` when it builds the wrapper.
2. **`skysaga-proto` has its own `BitStream`,** so nothing in Rust calls RakNet's. The only
   surface used is byte-oriented send and receive, which the bug never touched.

`libRakNet.so` exports 1879 unmangled `extern "C"` functions, so Rust calls them directly.
The roadmap's ~30-function shim became 19 declarations and no build step — no C++ toolchain,
no flake changes, nothing to keep in sync.

### The one trap: SWIG overload numbering

SWIG numbers overloads in declaration order, and the numbers are not guessable. Two bugs in
one sitting, both segfaults or silent no-ops rather than compile errors:

| wanted | wrong | right |
|---|---|---|
| `GetInternalID()` no-arg | `__SWIG_1` (takes a `SystemAddress`) | `__SWIG_2` |
| `AddressOrGUID(RakNetGUID)` | `__SWIG_2` (takes a `SystemAddress`) | `__SWIG_4` |

**Always check the generated C# in `server/Servers/SkySaga.RakNet/` before adding a binding.**
`RakPeerInterface.cs` and friends name each overload's parameters; `RakNetPINVOKE.cs` gives
the exact arity and types. Getting the arity wrong reads arguments off the stack.

A related one: the system-identifier argument to `Send` must be a real `AddressOrGUID` even
when broadcasting. RakNet dereferences it either way, so passing null is a segfault rather
than an ignored argument.

### Testing

`crates/raknet/tests/loopback.rs` runs two real peers over loopback UDP: handshake, addressed
send, broadcast, a 60 KB payload through RakNet's split/reassembly, and ordering. Peers bind
port 0 so the suite never collides with a running emulator.

Every test polls until the expected packet arrives rather than sleeping — RakNet works on its
own threads. Note that a handshake has two halves: the connecting side sees
`CONNECTION_REQUEST_ACCEPTED` slightly before the listening side sees
`NEW_INCOMING_CONNECTION`, so a helper that waits for only the first leaves the server's
connection table momentarily empty.

### What is still missing for a game server

The transport moves bytes; it does not yet run a game. Still to write:

- a connection registry mapping `Guid` to player state,
- a tick loop that **drains** `receive()` until empty (the C# takes one packet per 30 ms tick,
  a ~33 packet/s ceiling for the entire server),
- the inbound dispatch `match` on `PacketId`,
- the handshake packets (`ServerInfo`, `MapDefinition`, `ChunkSync`, `EntityAdd`, …).

The character-creation handlers are ready and tested and need only dispatching:
`SaveCharacterName` → `set_character_name` → reply `CharacterSaved`;
`CreateHomeworld` → `set_home_biome` → reply `HomeworldCreated`;
`SetCharacterCustomisationData` → `set_appearance`.
