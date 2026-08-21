#!/usr/bin/env python3
"""Dump the resource table the 2017 client polls while it sits in LOAD_GAME_OBJECTS.

The client's main thread spins in `FUN_005dff80`, a status lookup keyed by a 16-bit id:

    mov  eax, [edi + 0x30]              ; table base
    lea  ecx, [ebx + ebx*4]             ; id * 5
    cmp  byte [eax + ecx*8 + 0xf], 2    ; entry = table + id*40, state byte at +0xF
    lea  esi, [eax + ecx*8]
    jge  ...                            ; state >= 2 takes one path
    cmp  dword [esi], 0                 ; a null first word means "no such entry"
    je   ...

So entries are 40 bytes, the state is a byte at +0xF, and +0x00 is a pointer. Tracing the
function showed `this` (ecx) stable at one address, which is what --manager takes.

Reading the states directly is far more informative than tracing the poll: the poll fires
hundreds of thousands of times across dozens of ids and only proves the client is busy. What
matters is which entries never reach the ready state.

    uv run tools/resource-states-b36731.py --manager 0x10022ee0
    uv run tools/resource-states-b36731.py --manager 0x10022ee0 --watch 30

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import collections
import re
import subprocess
import sys
import time

ENTRY_SIZE = 40
STATE_OFFSET = 0xF
TABLE_POINTER = 0x30

PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{3,}")

parser = argparse.ArgumentParser()
parser.add_argument("--manager", required=True, help="the poll's `this`, e.g. 0x10022ee0")
parser.add_argument("--count", type=int, default=128, help="entries to walk")
parser.add_argument("--watch", type=float, default=0, help="re-read for this many seconds")
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


def text_at(addr, depth=2):
    """Any printable run at `addr`, or at what it points to."""
    if addr is None or addr not in PLAUSIBLE:
        return ""

    found = ASCII_RUN.search(read(addr, 64))

    if found:
        return found.group().decode()

    if depth:
        return text_at(word(addr), depth - 1)

    return ""


manager = int(args.manager, 0)
table = word(manager + TABLE_POINTER)

if not table:
    sys.exit(f"no table pointer at {manager + TABLE_POINTER:#010x}")

print(f"pid {pid}  manager {manager:#010x}  table {table:#010x}")


def snapshot():
    states = {}

    for index in range(args.count):
        entry = table + index * ENTRY_SIZE
        first = word(entry)
        raw = read(entry + STATE_OFFSET, 1)

        if not raw or first is None:
            continue

        states[index] = (raw[0], first)

    return states


states = snapshot()
histogram = collections.Counter(state for state, _ in states.values())

print(f"\n{len(states)} entries; state histogram: {dict(sorted(histogram.items()))}\n")
print(f"{'id':>4}  {'state':>5}  {'+0x00':>10}  what")

for index, (state, first) in sorted(states.items()):
    if first == 0:
        continue

    flag = "  <-- not ready" if state < 2 else ""
    print(f"{index:4}  {state:5}  {first:#010x}  {text_at(first)[:48]}{flag}")

if args.watch:
    print(f"\nwatching {args.watch}s for changes...")
    deadline = time.time() + args.watch

    while time.time() < deadline:
        current = snapshot()

        for index, (state, first) in sorted(current.items()):
            was = states.get(index, (None, None))[0]

            if was is not None and was != state:
                print(f"  id {index}: state {was} -> {state}")

        states = current
        time.sleep(0.5)

    print("  no further changes" if states else "")

mem.close()
