# Game data

The server reads two files from this directory (or from wherever `SKYSAGA_DATA_DIR` points):

| File | What it is |
|---|---|
| `Entities.json` | every entity's components and sync indices; the world cannot be built without it |
| `geodata.json` | block types, stack limits and loot tables |

Both belong to the game, not to this emulator, so neither is in this repository and neither
can be shipped in the container image. Copy them here from the C# emulator's tree
(`Data/Entities.json` and `Bundled/<build>/geodata.json`) before `docker compose up`.
