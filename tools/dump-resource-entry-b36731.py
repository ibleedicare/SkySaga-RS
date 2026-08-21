#!/usr/bin/env python3
"""Dump whole entries from the client's resource table, to identify the one that never loads.

`resource-states-b36731.py` finds *which* entry is stuck (state 1 while everything else is 0 or
2). This prints the full 40-byte record for chosen ids so the stuck one can be compared against
ready ones field by field: whatever differs is what identifies the resource.

Entry layout, so far as the poll `FUN_005dff80` reveals it:

    +0x00  dword   pointer, the same value for every entry (a vtable)
    +0x0A  byte    flags, tested with `shr al, 1` then `test al, 1`
    +0x0F  byte    state: 2 = ready, 1 = in progress, 0 = not requested

    uv run tools/dump-resource-entry-b36731.py --manager 0x10022ee0 --ids 33,0,7

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import re
import subprocess
import sys

ENTRY_SIZE = 40
TABLE_POINTER = 0x30
PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{3,}")

parser = argparse.ArgumentParser()
parser.add_argument("--manager", required=True)
parser.add_argument("--ids", required=True, help="comma-separated entry ids")
args = parser.parse_args()

pids = subprocess.run(
    ["pgrep", "-f", "[S]kySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running")

pid = int(pids[0])
mem = open(f"/proc/{pid}/mem", "rb", 0)


def read(addr, size):
    try:
        mem.seek(addr)
        return mem.read(size)
    except Exception:  # noqa: BLE001
        return b""


def word(addr):
    raw = read(addr, 4)

    return int.from_bytes(raw, "little") if len(raw) == 4 else None


def probe(value):
    """What a field might point at, if anything."""
    if value not in PLAUSIBLE:
        return ""

    blob = read(value, 96)

    if not blob:
        return "<unmapped>"

    found = ASCII_RUN.search(blob)

    if found:
        return "text: " + found.group().decode()[:60]

    inner = word(value)

    if inner is not None and inner in PLAUSIBLE:
        deeper = ASCII_RUN.search(read(inner, 96))

        if deeper:
            return "->text: " + deeper.group().decode()[:60]

    return "opaque " + blob[:12].hex(" ")


manager = int(args.manager, 0)
table = word(manager + TABLE_POINTER)

print(f"pid {pid}  table {table:#010x}\n")

for token in args.ids.split(","):
    index = int(token)
    entry = table + index * ENTRY_SIZE
    blob = read(entry, ENTRY_SIZE)

    if len(blob) != ENTRY_SIZE:
        print(f"id {index}: unreadable")
        continue

    state = blob[0x0F]
    label = {0: "not requested", 1: "IN PROGRESS", 2: "ready"}.get(state, "?")

    print(f"=== id {index}  @ {entry:#010x}  state {state} ({label}) ===")
    print("   raw " + blob.hex(" "))

    for off in range(0, ENTRY_SIZE, 4):
        value = int.from_bytes(blob[off : off + 4], "little")
        print(f"   +{off:#04x}  {value:#010x}  {probe(value)}")

    print()

mem.close()
