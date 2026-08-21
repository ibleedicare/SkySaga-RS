#!/usr/bin/env python3
"""Show the raw words around each GeoData count, to infer the container layout.

`probe-geodata-tables-b36731.py` guessed at the neighbouring pointer and could not attribute a
table to an offset: the manager holds a long run of pointers, so a sliding window keeps finding
the same unrelated blocks. This prints the actual words instead of guessing, so the container's
shape can be read off rather than inferred.

Read-only. Needs kernel.yama.ptrace_scope=0.

    uv run tools/dump-geodata-container-b36731.py
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MGR_PTR = 0x01487BCC
WINDOW = 8  # words either side of the count

PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{3,}")

offsets = json.loads((ROOT / "tools" / "reader_offsets.json").read_text())

pids = subprocess.run(
    ["pgrep", "-f", "SkySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running — start one first")

mem = open(f"/proc/{int(pids[0])}/mem", "rb", 0)


def read(addr, size):
    try:
        mem.seek(addr)
        return mem.read(size)
    except Exception:  # noqa: BLE001
        return b""


def word(addr):
    raw = read(addr, 4)

    return int.from_bytes(raw, "little") if len(raw) == 4 else None


def peek(addr):
    """A one-line hint at what `addr` points at."""
    if addr is None or addr not in PLAUSIBLE:
        return ""

    blob = read(addr, 64)

    if not blob:
        return "<unmapped>"

    runs = [m.group().decode() for m in ASCII_RUN.finditer(blob)]

    if runs:
        return "text: " + ", ".join(runs[:3])

    # Maybe a pointer to text.
    first = int.from_bytes(blob[:4], "little")

    if first in PLAUSIBLE:
        inner = [m.group().decode() for m in ASCII_RUN.finditer(read(first, 64))]

        if inner:
            return "->text: " + ", ".join(inner[:3])

    return "opaque " + blob[:8].hex(" ")


mgr = word(MGR_PTR)

if not mgr:
    sys.exit("manager pointer null — GeoData not loaded yet")

print(f"manager = {mgr:#010x}\n")

for label, info in sorted(offsets.items(), key=lambda kv: kv[1]["off"] or 0):
    off = info["off"]

    if off is None:
        continue

    count = word(mgr + off)

    if not count or not 0 < count < 100_000:
        continue

    print(f"=== {label}  off={off:#06x}  count={count} ===")

    for delta in range(-WINDOW * 4, WINDOW * 4 + 1, 4):
        value = word(mgr + off + delta)

        if value is None:
            continue

        mark = "  <-- count" if delta == 0 else ""
        print(f"  {delta:+#07x}  {value:#010x}  {peek(value):<52}{mark}")

    print()

mem.close()
