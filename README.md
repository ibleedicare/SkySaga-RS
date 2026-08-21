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
| `SKYSAGA_CLIENT_BUILD` | `10414` | `36731` serves Alpha V10, translating every packet id |
| `SKYSAGA_WEB_HTTPS` | *(unset)* | `1` also listens with TLS, which Alpha V10 requires |
| `SKYSAGA_WEB_HTTPS_PORT` | `5165` | |
| `SKYSAGA_WEB_CERT` / `SKYSAGA_WEB_KEY` | *(generated)* | PEM paths, if you would rather supply your own |
| `RUST_LOG` | `info` | e.g. `skysaga_web=debug` to see every request body |

## TLS, for the 2017 client

Alpha V10 (b36731) calls the web API over **https**, and it cannot be talked out of it: the
scheme is a per-RPC flag compiled into the client, chosen at the URL builder `0x00c0eec0`, and
the login RPC is registered secure. Against a plain-http server the client never gets past the
login screen, showing `SERVER ERROR / 404`. Build 10414 is http-only and is unaffected by any
of this, which is why the two are served on separate ports and both clients can be online at
once:

```text
http  :5164   build 10414
https :5165   build 36731
```

### The certificate

The client does **not** verify the certificate, so a self-signed one is fine. It is generated
on first run and cached beside the binary as `skysaga-dev-cert.pem` / `skysaga-dev-key.pem`,
because a certificate that changed on every restart would be a new identity to the client each
time. Both files are gitignored: one of them is a private key.

To generate one yourself, or to replace a cached one:

```bash
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout skysaga-dev-key.pem -out skysaga-dev-cert.pem \
    -subj "/CN=127.0.0.1" \
    -addext "subjectAltName=IP:127.0.0.1,DNS:localhost"
```

Point `SKYSAGA_WEB_CERT` / `SKYSAGA_WEB_KEY` at them if they live anywhere else. Add the
address the client actually connects to (`SKYSAGA_PUBLIC_IP`) to `subjectAltName` when the
client is not on this host: it costs nothing and keeps the certificate usable if anything ever
does verify it.

**Use RSA, not ECDSA.** The C# emulator served RSA-2048 with `KeyEncipherment` key usage, which
also permits the old RSA key-exchange suites this client's era of OpenSSL expects.

### Terminating TLS in front, which is what currently works

**`SKYSAGA_WEB_HTTPS=1` does not yet get this client through the handshake.** The listener is
built on rustls, which implements neither RSA key exchange nor TLS below 1.2, and the 2017
client cannot negotiate with it: its connections reach the server and go straight to `TIME-WAIT`
with nothing logged, not even the catch-all unimplemented-route fallback, while `curl -k`
against the same listener returns 200. Swapping the certificate from ECDSA to RSA changed
nothing, so it is the protocol, not the certificate.

Until the TLS stack moves to something OpenSSL-backed, terminate it in front of the plain http
port. `openssl s_server` holds the connection where rustls drops it, and so does socat:

```bash
# serve the API on plain http somewhere the client is not pointed at
SKYSAGA_WEB_PORT=5166 SKYSAGA_GAME_PORT=42070 SKYSAGA_AUTH_PORT=10107 \
    cargo run --release -p skysaga-server

# then terminate TLS on the port the client does use
socat OPENSSL-LISTEN:5165,reuseaddr,fork,cert=skysaga-dev-cert.pem,key=skysaga-dev-key.pem,\
verify=0,cipher=ALL:@SECLEVEL=0,openssl-min-proto-version=TLS1 TCP:127.0.0.1:5166
```

`@SECLEVEL=0` and `openssl-min-proto-version=TLS1` are what let a modern OpenSSL speak to a
2017 client at all; both are needed. With that in front, the client logs in, fetches its active
character, and is handed the game server address.

Check it end to end before launching a client, since a dead web server looks exactly like a
protocol bug from the client side:

```bash
curl -s  -o /dev/null -w '%{http_code}\n' http://127.0.0.1:5166/ping   # 200
curl -sk -o /dev/null -w '%{http_code}\n' https://127.0.0.1:5165/ping  # 200
```

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
