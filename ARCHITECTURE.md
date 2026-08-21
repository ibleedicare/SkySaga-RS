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
