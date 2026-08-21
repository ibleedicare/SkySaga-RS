#!/usr/bin/env python3
"""Name every GeoData table a MapSpec slot indexes, by matching the client's own name hashes.

The GeoData manager is an array of **12-byte records**:

    +0  name hash   CRC-32 of the table's name, the client's usual `name_hash`
    +4  data ptr
    +8  entry count   <- what `read_counts_attach.py` reads

So the table four bytes before each known count pointer *names itself*, and no string walking
or dumper reversing is needed: hash the candidate names and compare. §8 gave each MapSpec slot
its count offset; this turns that into "slot 3 indexes <table>", which is what we need in order
to send a valid index instead of zero.

Candidate names come from the 10414 `geodata.json`, which is a different build — a name that
fails to match may simply not exist in 2017, and unmatched hashes are reported rather than
hidden.

Read-only. Needs kernel.yama.ptrace_scope=0.

    uv run tools/name-geodata-tables-b36731.py
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MGR_PTR = 0x01487BCC
# Every bundled GeoData, newest-era first. `36546` is the closest build to 36731 by far, so its
# table names are the most likely to be the ones this client loaded.
GEODATA_DIRS = ROOT / "server" / "Bundled"

# --- the client's name hash ------------------------------------------------------------------
# CRC-32, polynomial 0x04C11DB7, MSB-first, not reflected, init 0, no final XOR, input
# lowercased. Ported from SkySaga.Game/Util.cs::ComputeCrc32; mirrors skysaga_core::name_hash.

def build_table():
    table = []

    for index in range(256):
        entry = index << 24

        for _ in range(8):
            entry = ((entry << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if entry & 0x80000000 else (entry << 1) & 0xFFFFFFFF

        table.append(entry)

    return table


TABLE = build_table()


def name_hash(name):
    crc = 0

    for byte in name.lower().encode():
        crc = TABLE[((crc >> 24) ^ byte) & 0xFF] ^ ((crc << 8) & 0xFFFFFFFF)

    return crc


assert name_hash("Sky_Island") == 0xCBBFA7BF, "name_hash disagrees with the Rust doctest"

# --- candidate names --------------------------------------------------------------------------

candidates = set()
sources = sorted(GEODATA_DIRS.glob("*/[Gg]eo[Dd]ata.json"))

for source in sources:
    try:
        data = json.loads(source.read_text())
    except Exception as error:  # noqa: BLE001
        print(f"note: {source} unreadable ({error})", file=sys.stderr)
        continue

    if not isinstance(data, dict):
        continue

    candidates.update(data.keys())

    # A table may be addressed by its singular type name as well as its plural key, and the
    # nested keys are where the per-entry type names live.
    for key, value in data.items():
        if key.endswith("s"):
            candidates.add(key[:-1])

        if isinstance(value, list) and value and isinstance(value[0], dict):
            candidates.update(value[0].keys())

print(f"candidate sources: {', '.join(s.parent.name for s in sources) or 'none'}", file=sys.stderr)

# Names §8 recovered from the client's own MapSpec dumper, which are the ones that matter most.
candidates.update(
    """version searchable nonSearchable masteryLevels biome region adventure difficulty
    adventureType seed featureCreatureSet terrainCreatureSet caveCreatureSet timeOfDayPreset
    timeOfDayPresetList mapSizeCategory mapSize terrainGenerator featureName activeEvent
    forcedMap cost Biomes Regions Adventures Difficulties Palettes CreatureSets TimeOfDayPresets
    MapSizes TerrainGenerators Features Events Maps""".split()
)

by_hash = {name_hash(name): name for name in candidates}

# --- read the manager -------------------------------------------------------------------------

offsets = json.loads((ROOT / "tools" / "reader_offsets.json").read_text())

pids = subprocess.run(
    ["pgrep", "-f", "SkySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running — start one first")

pid = int(pids[0])
mem = open(f"/proc/{pid}/mem", "rb", 0)


def word(addr):
    try:
        mem.seek(addr)
        raw = mem.read(4)
    except Exception:  # noqa: BLE001
        return None

    return int.from_bytes(raw, "little") if len(raw) == 4 else None


mgr = word(MGR_PTR)

if not mgr:
    sys.exit("manager pointer null — GeoData not loaded yet")

print(f"attached to pid {pid}, manager = {mgr:#010x}")
print(f"{len(candidates)} candidate names\n")
print(f"{'slot':16} {'offset':>8} {'count':>6}  {'hash':>10}  table")

report = {}

for label, info in sorted(offsets.items(), key=lambda kv: kv[1]["off"] or 0):
    off = info["off"]

    if off is None:
        continue

    count = word(mgr + off)
    digest = word(mgr + off - 8)

    if not count or not 0 < count < 100_000 or digest is None:
        continue

    name = by_hash.get(digest, "<unmatched>")

    print(f"{label:16} {off:#8x} {count:6}  {digest:#010x}  {name}")

    report[label] = {"offset": off, "count": count, "hash": digest, "table": name}

mem.close()

out = ROOT / "logs" / "geodata-table-names-b36731.json"
out.write_text(json.dumps(report, indent=1))

matched = sum(1 for entry in report.values() if entry["table"] != "<unmatched>")
print(f"\nnamed {matched} of {len(report)}; wrote {out}")
