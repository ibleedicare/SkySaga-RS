#!/usr/bin/env python3
"""List what the 2017 client is still waiting to load before it will enter the world.

`FUN_007a4800` decides the readiness byte the whole handshake hangs on:

    if (1 < *(int *)(this + 0xa4)) {                 // level state is at least 2
        *(byte *)(this + 0x51a4) = 1;                // assume ready
        for (entry in [DAT_0143785c+0x12a8 .. +0x12ac], stride 12) {
            if (*(int *)(entry + 0x20) == 0)  ready = 0;    // this one is not loaded
            else if (...state 1 checks...)    ready = 0;
        }
    }

So readiness is "every entry in that list is loaded", and one entry being unfinished is what
keeps the client in LOAD_GAME_OBJECTS forever. This walks the list and prints each entry's
state, with any text found near it, so the unfinished one can be identified.

`DAT_0143785c` is the loading-screen global; its value has been 0x10232f90 in every observed
run. Pass --app to override.

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import re
import subprocess
import sys

LIST_BEGIN = 0x12A8
LIST_END = 0x12AC
STRIDE = 12
STATE_OFFSET = 0x20

PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{4,}")

parser = argparse.ArgumentParser()
parser.add_argument("--app", default="0x10232f90", help="value of DAT_0143785c")
args = parser.parse_args()

pids = subprocess.run(
    ["pgrep", "-f", "[S]kySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running")

pid = int(pids[0])
app = int(args.app, 0)
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


def text_near(addr, depth=2):
    """Any printable run at `addr`, or at what its first few fields point to."""
    if addr is None or addr not in PLAUSIBLE:
        return ""

    found = ASCII_RUN.search(read(addr, 64))

    if found:
        return found.group().decode()[:48]

    if depth:
        for slot in range(0, 24, 4):
            deeper = text_near(word(addr + slot), depth - 1)

            if deeper:
                return deeper

    return ""


begin = word(app + LIST_BEGIN)
end = word(app + LIST_END)

if begin is None or end is None or begin > end:
    sys.exit(f"list pointers look wrong: begin={begin}, end={end}")

count = (end - begin) // STRIDE

print(f"pid {pid}, app {app:#010x}")
print(f"list {begin:#010x}..{end:#010x}  ({count} entries)\n")
print(f"{'#':>3}  {'entry':>10}  {'+0x20':>6}  what")

pending = 0

for index in range(count):
    slot = begin + index * STRIDE
    entry = word(slot)

    if entry is None:
        continue

    state = word(entry + STATE_OFFSET)
    flag = word(slot + 8)
    mark = ""

    if state == 0:
        mark = "   <-- NOT LOADED"
        pending += 1

    print(f"{index:3}  {entry:#010x}  {state:6}  {text_near(entry)[:44]}{mark}  flag={flag}")

print(f"\n{pending} of {count} entries are not loaded")

mem.close()
