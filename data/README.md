# Game data

The server reads these from this directory, or from wherever `SKYSAGA_DATA_DIR` points:

| File | What it is |
|---|---|
| `entities.json` | every entity's components and sync indices; the world cannot be built without it |
| `geodata.json` | block types, stack limits and loot tables |

They belong to the game, not to this project, so this repository does not carry them and
neither does the container image. Produce them from your own copy of the game:

```bash
# with a Rust toolchain
cargo run --release -p skysaga-datapc -- "/path/to/SkySaga Infinite Isles/Client" data/

# or with only Docker
SKYSAGA_CLIENT_DIR="/path/to/SkySaga Infinite Isles/Client" docker compose run --rm extract
```

Both read the client's `Data/data.pc` and write the files out here. Neither writes to the
client. [`crates/skysaga-datapc`](../crates/skysaga-datapc/src/lib.rs) documents the
container format.

## Why this repository does not ship them

These are unmodified game files. The extractor changes nothing but line endings, so every
file matches a member of `data.pc` byte for byte. Extraction copies an existing work rather
than making a new one. Copyright belongs to whoever holds SkySaga's rights, and nothing
grants permission to pass them on.

Would anyone act on a cancelled game whose servers are gone? Probably not. This project still
does not take the bet, for the same reason OpenMW and OpenRA do not. Shipping the extractor
instead costs one command.

## A note on spelling

The archive calls it `entities.json`. The C# emulator's tree and most of this codebase say
`Entities.json`. The server accepts either. On Windows the difference never came up, and on
Linux insisting on one spelling would fail to find a perfectly good copy of the other.
