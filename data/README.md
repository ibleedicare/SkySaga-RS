# Game data

The server reads these from this directory, or from wherever `SKYSAGA_DATA_DIR` points:

| File | What it is |
|---|---|
| `entities.json` | every entity's components and sync indices; the world cannot be built without it |
| `geodata.json` | block types, stack limits and loot tables |

They are the game's own data files, not this project's, so they are not in this repository
and are not baked into the container image. Produce them from your own copy of the game:

```bash
# with a Rust toolchain
cargo run --release -p skysaga-datapc -- "/path/to/SkySaga Infinite Isles/Client" data/

# or with only Docker
SKYSAGA_CLIENT_DIR="/path/to/SkySaga Infinite Isles/Client" docker compose run --rm extract
```

Both read the client's `Data/data.pc` and write the files out here. The client is opened
read-only. See [`crates/skysaga-datapc`](../crates/skysaga-datapc/src/lib.rs) for the
container format.

## Why they are not shipped

They are unmodified game files — verifiably so: every one is byte-identical to a member of
`data.pc` once CRLF is folded to LF. Copyright in them belongs to whoever holds SkySaga's
rights, and nothing grants permission to redistribute them. That extracting them yourself is
easy does not change who owns them.

Whether anyone would ever act on a cancelled game whose servers are gone is a separate
question, and the honest answer is probably not. This project does not take the bet, for the
same reason OpenMW and OpenRA do not: shipping the extractor instead costs one command.

## A note on spelling

The archive calls it `entities.json`; the C# emulator's tree and most of this codebase say
`Entities.json`. Both are accepted — on Windows the difference never mattered, and on Linux
insisting on one would fail to find a perfectly good copy of the other.
