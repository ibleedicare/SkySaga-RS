# SkySaga server emulator — Rust port

A rewrite of the C# emulator in `../server/Servers/`. See [ARCHITECTURE.md](ARCHITECTURE.md)
for the design and why it is shaped the way it is.

**Status: stage 1 complete — a client logs in and reaches the world against this server.**
Verified with the real client (build 10414) on 2026-08-21, with the C# web and auth servers
stopped. The game server (RakNet, UDP :42069) is still the C# one; that is stage 2.

```
crates/
  skysaga-core/     name hashing, bit maths, fixed-width strings, byte reader   (pure)
  skysaga-state/    accounts, sessions, characters                              (pure)
  skysaga-auth/     Smilegate login   TCP :10106
  skysaga-web/      account / characters / conductor / social   HTTP :5164
  skysaga-server/   one binary running them all over one shared state
```

## Running it

```bash
cargo run --release -p skysaga-server
```

Then start the C# game server and launch the client as usual:

```bash
cd ../server/Servers/SkySaga.Game/bin/Release/net9.0 && dotnet SkySaga.Game.dll
nix run ..#sky-saga
```

Environment variables are unchanged from the C#, so the existing scripts work as they are:

| Variable | Default | |
|---|---|---|
| `SKYSAGA_ACCOUNTS` | *(unset)* | `user:pass,other:pass` to restrict logins; unset accepts any non-empty name |
| `SKYSAGA_PUBLIC_IP` | `127.0.0.1` | address advertised to the client — set it when the client is not on this host |
| `SKYSAGA_WEB_PORT` | `5164` | |
| `SKYSAGA_AUTH_PORT` | `10106` | |
| `SKYSAGA_GAME_PORT` | `42069` | address handed out by `game-conductor/retrieve` |
| `RUST_LOG` | `info` | e.g. `skysaga_web=debug` to see every request body |

## Tests

```bash
cargo test --workspace          # 72 tests, no network, no fixtures directory to prepare
```

The tests are the point of the rewrite, so a word on what they actually check.

**The C# server is the oracle, not this code's own opinion.** The fixtures under
`crates/*/tests/golden/` were captured by running the real C# servers and recording what they
put on the wire — `scripts/capture-auth-golden.py` for the binary login protocol, `curl` for
the HTTP API. A test passes when this server's output matches *the C#'s*, not when it matches
what the port's author believed the format to be.

That distinction caught real things. `LoginRequest`'s layout is confirmed because the C#
server parsed a packet this crate encoded and echoed the username back; had a field offset
been wrong, it would have echoed garbage.

**Response keys are compared, not just values.** The client reads `GUID`, `RESERVED_NAME` and
`Error` case-sensitively while everything around them is lower-case — the C# had to disable
ASP.NET's camelCase policy wholesale after `RESERVED_NAME` went out as `reserveD_NAME`. A
test that only checked values would not notice.

**HTTP tests do not bind a port.** Requests go through the router with
`tower::ServiceExt::oneshot`, so the suite runs in well under a second and never races.

## Improvements over the C#

Defects found while reading the original, fixed here rather than reproduced:

- **The auth server assumed one `read()` returned the whole packet**
  (`SmilegateAuth/Program.cs:22`). TCP may split it anywhere; there is a test that sends the
  header, pauses, then dribbles the body two bytes at a time.
- **The auth server handled one connection at a time.** A client that connected and then
  stalled blocked every other player from signing in.
- **`Session.AccountName` and `_characterUUID` were process-wide statics**, so the emulator
  served exactly one player and two clients corrupted each other. State is keyed per account
  here, and requests are attributed by peer address — the client sends nothing else that
  identifies it (no `Authorization` header, no id in the path).
- **Three processes sharing no state.** One process, one `Arc<AppState>`, so the game server
  can see what the web server knows instead of re-deriving it.

## Known gaps

- **The account name is whatever the client's application login sends** — with `nix run
  .#sky-saga` that is `projectv-client`, not a player name, because those launch variables do
  not include `auth`. This matches the C#'s behaviour exactly; it is parity, not a
  regression. Launching with the `auth` variable set routes through `sgauth/_login` and gives
  the real name.
- **The social graph returns empty lists.** The response *shapes* are implemented (they are
  the part that is easy to get wrong), but the interactive friends/requests/blocked graph the
  C# grew is not ported.
- **No HTTPS yet.** The 2017 builds (Alpha V10 b36731) need it on :5165; retail 10414, which
  is what this was verified against, is plain HTTP.
- **Photos, trading and binary storage are unimplemented** — they log as unhandled routes.
- **No RakNet.** Stage 2.

## Contributing

The whole layout is chosen so that adding something is adding a file:

- **An HTTP endpoint** → one `async fn` plus one `.route()` line in that module's `router()`.
- **A packet** *(stage 2)* → one file in `skysaga-proto/src/packets/`, one `PacketId` variant,
  one arm in the dispatch `match`. The compiler tells you if you miss the arm.

Write the test first, and where behaviour must match the C#, capture what the C# does rather
than asserting what you think it does.
