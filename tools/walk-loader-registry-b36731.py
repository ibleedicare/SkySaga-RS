#!/usr/bin/env python3
"""Walk the registry a stalled loader searches, to see whether its item is there at all.

From `FUN_004b6dd0`, a loader with a name reaches state 2 only if it finds a matching item:

    key = intern(name at loader+0x64)
    for (node = *(loader[0xd] + 0xb8); node; node = node[1])
        item = node[0]
        if (item && item[0x11] == key) -> found, state = 2

`loader[0xd]` is `+0x34`, a per-loader world object, and `+0xB8` is that object's item list.
So if `w_Trees`'s list is empty, or holds nothing whose key matches, the loader stays at 1 and
the whole handshake stalls behind it.

    uv run tools/walk-loader-registry-b36731.py --loader 0x130d8930 --label w_Trees
    uv run tools/walk-loader-registry-b36731.py --loader 0x130d8830 --label w_ScatterAssets

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import re
import subprocess
import sys

WORLD_OFFSET = 0x34
LIST_OFFSET = 0xB8
KEY_OFFSET = 0x44

PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{3,}")

parser = argparse.ArgumentParser()
parser.add_argument("--loader", required=True)
parser.add_argument("--label", default="")
parser.add_argument("--limit", type=int, default=40)
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


def text(addr, span=64):
    if addr is None or addr not in PLAUSIBLE:
        return ""

    found = ASCII_RUN.search(read(addr, span))

    return found.group().decode()[:40] if found else ""


loader = int(args.loader, 0)
world = word(loader + WORLD_OFFSET)
state = word(loader + 0x20)

# The name is stored inline in the loader; show it so the label cannot be wrong.
inline = ASCII_RUN.search(read(loader + 0x3C, 32))

print(f"pid {pid}  loader {loader:#010x} {args.label}")
print(f"  inline name : {inline.group().decode() if inline else '?'}")
print(f"  state +0x20 : {state}")
print(f"  world +0x34 : {world:#010x}" if world else "  world: null")

if not world:
    sys.exit(0)

head = word(world + LIST_OFFSET)

print(f"  list  +0xb8 : {head:#010x}" if head else "  list  +0xb8 : EMPTY (null head)")

node = head
count = 0

while node and count < args.limit:
    item = word(node)
    key = word(item + KEY_OFFSET) if item else None

    print(f"    [{count:2}] node {node:#010x}  item {item:#010x}  key {key:#010x}  {text(item)}"
          if item else f"    [{count:2}] node {node:#010x}  item null")

    node = word(node + 4)
    count += 1

print(f"  {count} items")

mem.close()
