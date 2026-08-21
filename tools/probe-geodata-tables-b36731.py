#!/usr/bin/env python3
"""Find how b36731's GeoData tables store their entries, so each MapSpec slot can be named.

`read_counts_attach.py` reads each table's *count* at `[0x1487bcc] + off`. That gives every
MapSpec field's bit width, which is what §8 needed. It does not say **what** the table holds —
and that is what stops us choosing a valid index to send.

The tables identify themselves if we can reach their entries: a table holding `Sky_Island` /
`Desert` is Biomes, one holding `Home_Island_Adventure` is Adventures. So this walks outwards
from each known count offset looking for the neighbouring data pointer, follows it, and prints
any ASCII it finds.

Nothing here is assumed about the container layout — the point is to *discover* it. Read-only:
opens /proc/<pid>/mem for reading and never writes.

Needs kernel.yama.ptrace_scope=0.

    uv run tools/probe-geodata-tables-b36731.py
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MGR_PTR = 0x01487BCC

# How far either side of the count to look for the pointer that goes with it.
NEIGHBOURHOOD = 0x20

# A 32-bit userspace address in this process looks like this. Wine maps the PE low and the
# heap higher; anything outside is not worth dereferencing.
PLAUSIBLE = range(0x00010000, 0xC0000000)

ASCII_RUN = re.compile(rb"[ -~]{4,}")

offsets = json.loads((ROOT / "tools" / "reader_offsets.json").read_text())

pids = subprocess.run(
    ["pgrep", "-f", "SkySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running — start one first")

pid = int(pids[0])
mem = open(f"/proc/{pid}/mem", "rb", 0)

print(f"attached to pid {pid}")


def read(addr, size):
    try:
        mem.seek(addr)
        return mem.read(size)
    except Exception:  # noqa: BLE001 -- an unmapped address is a normal outcome here
        return b""


def word(addr):
    raw = read(addr, 4)

    return int.from_bytes(raw, "little") if len(raw) == 4 else None


def strings_at(addr, size=256):
    """Any printable runs in `size` bytes at `addr`."""
    return [m.group().decode() for m in ASCII_RUN.finditer(read(addr, size))]


def describe(addr, depth=2):
    """ASCII at `addr`, or at whatever `addr` points to, up to `depth` hops."""
    found = strings_at(addr)

    if found or depth == 0:
        return found

    # Not text: maybe an array of pointers to text.
    for slot in range(0, 32, 4):
        target = word(addr + slot)

        if target is not None and target in PLAUSIBLE:
            found += describe(target, depth - 1)

            if found:
                return found

    return found


mgr = word(MGR_PTR)

if not mgr:
    sys.exit("manager pointer null — GeoData not loaded yet, wait longer")

print(f"[{MGR_PTR:#010x}] = {mgr:#010x}  (GeoData manager)\n")

report = {}

for label, info in sorted(offsets.items(), key=lambda kv: kv[1]["off"] or 0):
    off = info["off"]

    if off is None:
        continue

    count = word(mgr + off)

    if not count or not 0 < count < 100_000:
        print(f"{label:16} off={off:#06x}  count implausible ({count}) — skipped")
        continue

    print(f"{label:16} off={off:#06x}  count={count}")

    # Walk the neighbourhood for a pointer whose target yields text.
    for delta in range(-NEIGHBOURHOOD, NEIGHBOURHOOD + 1, 4):
        if delta == 0:
            continue

        candidate = word(mgr + off + delta)

        if candidate is None or candidate not in PLAUSIBLE:
            continue

        found = describe(candidate)

        if found:
            sample = ", ".join(found[:6])
            print(f"    {delta:+#06x} -> {candidate:#010x}  {sample}")
            report.setdefault(label, []).append(
                {"delta": delta, "ptr": candidate, "strings": found[:12]}
            )

    if label not in report:
        print("    no neighbouring pointer produced text")

    print()

mem.close()

out = ROOT / "logs" / "geodata-tables-b36731.json"
out.write_text(json.dumps(report, indent=1))
print(f"wrote {out}")
