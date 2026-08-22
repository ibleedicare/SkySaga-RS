# SkySaga server emulator: Rust architecture

A rewrite in Rust of the **C# SkySaga emulator by EDITz**,
[EDITzDev/SkySaga](https://github.com/EDITzDev/SkySaga) (~33k LOC across four projects), used
under the MIT licence. That project is where this one starts from: the protocol it speaks, the
behaviour it reproduces and the tests it is checked against all come from there. This port is
a derivative work of it and carries its copyright notice — see [LICENSE](LICENSE).

This document is the *design contract*. The C# server is the behavioural oracle: when this
document and the C# disagree about bytes on the wire, the C# wins and this document is wrong.
When they disagree about *structure*, this document wins. The point of the rewrite is that
the structure is contributable.

References to the C# tree below mean a checkout of that repository, which this one does not
contain.

---

## Shape of the port

```
rust-server-port/
├── Cargo.toml                workspace
├── ARCHITECTURE.md           this file
└── crates/
    ├── skysaga-core/         pure values: name hashing, bit maths, fixed-width strings, ids
    ├── skysaga-proto/        RakNet packet wire formats, encode/decode, no I/O
    ├── skysaga-world/        entities, components, chunks, pure simulation, no I/O
    ├── skysaga-state/        the shared world-independent state: accounts, characters, sessions
    ├── skysaga-store/        persistence: the Store trait, and SQLite
    ├── skysaga-auth/         Smilegate login   (TCP  :10106)
    ├── skysaga-web/          account / conductor / social API   (HTTP :5164, HTTPS :5165)
    ├── skysaga-chat/         the client's IRC dialect, and a server for it  (TCP :4444)
    ├── skysaga-game/         game server: joins net ↔ world   (UDP :42069)
    ├── raknet/               safe wrapper over…
    ├── raknet-sys/           …the SLikeNet C API
    ├── skysaga-probe/        a headless client, for tests that need a real socket
    ├── skysagactl/           command-line control of a running server
    └── skysaga-server/       one binary that runs all of them over one shared state
```

The four pure crates (`core`, `proto`, `world`, `state`) are listed first because everything
else depends on them and they depend on nothing.

### Rule 1: I/O lives at the edges

`skysaga-core`, `skysaga-state`, `skysaga-proto` and `skysaga-world` have **no** sockets, no
files, no clock, no `tokio`. Everything they do is a function from values to values, which
means everything they do is unit-testable without a client, a network or a fixture directory.

`auth`, `web` and `game` are thin: they move bytes in, call a pure function, move bytes out.

This is the single change that makes the project testable. `Connection.cs` cannot be tested
because there is no seam between "read a packet" and "decide what happens"; here there always
is one.

### Rule 2: one process, one shared state

The C#'s three processes are an accident of how it was built, and it costs correctness: the
game server has to re-derive the account name that the web server already knew.

`skysaga-server` is one binary running auth, web, chat and game as `tokio` tasks over one
`Arc<AppState>`. The individual binaries (`skysaga-auth`, `skysaga-web`, `skysaga-game`) stay
runnable on their own, so a contributor can work on one piece and so a Rust service can be run
against the C# for comparison.

### Rule 3: no global mutable state, ever

`AppState` holds `accounts: RwLock<HashMap<AccountId, Account>>` and friends. Multiple
players work by construction rather than by later effort. Nothing is `static mut`, nothing is
a `lazy_static` singleton, nothing is "single-slot because the emulator serves one player".

### Rule 4: adding a thing is adding a file

The contribution test for every subsystem:

- **A new HTTP endpoint** → one `async fn` + one `.route()` line in that module's `router()`.
- **A new packet** → one struct with a `const ID` and an `encode`/`decode` in
  `skysaga-proto/src/packets/`, one variant on `ClientPacket`, one arm in the dispatch `match`.
  The compiler tells you if you miss the arm.
- **A new component** → one variant on `enum Component` + its `sync()` arm. Exhaustive
  `match` means a missing case is a compile error, not a silent no-op.

No reflection, no registries keyed by string, no `Activator.CreateInstance`. If it can be an
exhaustive `match`, it is one.

### Rule 5: the wire format is written down, not inferred

C#'s `[StructLayout(Pack = 1)]` marshalling means the layout of `LoginReply` exists only as a
consequence of field declaration order. Here every wire struct has an explicit
`const SIZE: usize`, an explicit field-offset table in a doc comment, and a test asserting a
known-good byte sequence. Layout is stated, then verified.

---

## Testing strategy (TDD, in the order tests are written)

1. **Unit tests, pure crates.** Written *before* the implementation. Known-good vectors come
   from the C#: CRC32 hashes, packet sizes, byte-for-byte encodings.
2. **Golden-byte tests.** Fixtures recorded from the real thing: `tests/golden/` in `auth`
   and `web`, `tests/fixtures/*.tsv` in `proto`. A wrong integer width fails a test in
   milliseconds instead of appearing as "client stuck at Ready to Play".
3. **HTTP contract tests.** Each endpoint is exercised in-process through
   `tower::ServiceExt::oneshot`: no port binding, no sleep, fast. Assertions are on the exact
   JSON keys, because the client reads them case-sensitively (`RESERVED_NAME` must not become
   `reserveD_NAME`, a real bug the C# had to work around).
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
  for the entire server. Drain until empty.
- **`ToFrozenSet()` rebuilt every tick** for structures that are build-once.

---

## Ports

Unchanged from the C# so the existing launch scripts keep working:

| Service | Port | |
|---|---|---|
| web | 5164/tcp | http, build 10414 |
| web | 5165/tcp | https, build 36731 (`SKYSAGA_WEB_HTTPS=1`) |
| auth | 10106/tcp | Smilegate login |
| game | 42069/udp | RakNet |

---

## Addendum: `skysaga-proto`

`skysaga-proto` landed ahead of the RakNet transport, because the wire formats are pure
functions and can be tested without one. It holds:

- `bitstream`: a pure-Rust reimplementation of the subset of RakNet's `BitStream` the game
  uses, verified byte-for-byte against the real `libRakNet.so`.
- `customisation`: `CustomisationData`: gender, tribe, skin/eye/clothing materials and the
  hair attachment.
- `packets`: one module per area of the protocol — the handshake, character creation,
  movement, inventory, voxels, interaction, chat, mail, photos, combat.

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

It needs a checkout of the upstream C# tree and `libRakNet.so`; it is a development tool, not
part of the server. Writing the packet's shape into that generator *first*, then making Rust
reproduce its output, is the workflow. It is what stops the tests from merely agreeing with
the Rust.

---

## Addendum: the RakNet transport

Two crates, and they are the only place `unsafe` appears in the port:

- `raknet-sys`: 26 `extern "C"` declarations, no code.
- `raknet`: a safe wrapper: `Peer`, `Packet`, `Guid`, with `Drop` on both handles.

### No C++ shim, after all

The roadmap called for a hand-written C++ shim over SLikeNet, on the grounds that the SWIG
wrapper carries the LP64 `long` bug (`Write< long >` is 32 bits on MSVC, 64 on Linux). Two
things retired that plan:

1. The flake already narrows `long` to `int32_t` when it builds the wrapper.
2. **`skysaga-proto` has its own `BitStream`,** so nothing in Rust calls RakNet's. The only
   surface used is byte-oriented send and receive, which the bug never touched.

`libRakNet.so` exports 1879 unmangled `extern "C"` functions, so Rust calls them directly.
The roadmap's ~30-function shim became 26 declarations and no build step: no C++ toolchain,
no flake changes, nothing to keep in sync.

### The one trap: SWIG overload numbering

SWIG numbers overloads in declaration order, and the numbers are not guessable. Two bugs in
one sitting, both segfaults or silent no-ops rather than compile errors:

| wanted | wrong | right |
|---|---|---|
| `GetInternalID()` no-arg | `__SWIG_1` (takes a `SystemAddress`) | `__SWIG_2` |
| `AddressOrGUID(RakNetGUID)` | `__SWIG_2` (takes a `SystemAddress`) | `__SWIG_4` |

**Always check the generated C# in the upstream tree's `Servers/SkySaga.RakNet/` before adding
a binding.**
`RakPeerInterface.cs` and friends name each overload's parameters; `RakNetPINVOKE.cs` gives
the exact arity and types. Getting the arity wrong reads arguments off the stack.

A related one: the system-identifier argument to `Send` must be a real `AddressOrGUID` even
when broadcasting. RakNet dereferences it either way, so passing null is a segfault rather
than an ignored argument.

### Testing

`crates/raknet/tests/loopback.rs` runs two real peers over loopback UDP: handshake, addressed
send, broadcast, a 60 KB payload through RakNet's split/reassembly, and ordering. Peers bind
port 0 so the suite never collides with a running emulator.

Every test polls until the expected packet arrives rather than sleeping. RakNet works on its
own threads. Note that a handshake has two halves: the connecting side sees
`CONNECTION_REQUEST_ACCEPTED` slightly before the listening side sees
`NEW_INCOMING_CONNECTION`, so a helper that waits for only the first leaves the server's
connection table momentarily empty.
