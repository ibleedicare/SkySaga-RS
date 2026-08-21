#!/usr/bin/env python3
"""Read, watch, or force the 2017 client's loading stage.

The client's loading screen keeps its current stage as a **byte at `object + 0xD34`**, indexing
the name array at `0x013fc1e8`:

    0 LOAD_GEO   1 STARTUP   2 CONFIG_GRAPHICS   3 WAIT_FOR_SERVER   4 CONNECT_TO_SERVER
    5 LOAD_GAME_OBJECTS   6 LOAD_EDITOR   7 DOWNLOAD_WORLD   8 POPULATE_WORLD
    9 TELEPORTING  10 EDITOR  11 VIEWER  12 LEAVING_WORLD  13 READY_TO_PLAY
   14 CHARACTER_CREATION

The object address is not fixed: get it by breakpointing 0x008f7a7a, where the loading text is
composed, and reading `eax`. Pass it here with --object.

    uv run tools/loading-stage-b36731.py --object 0x10232f90
    uv run tools/loading-stage-b36731.py --object 0x10232f90 --watch 60
    uv run tools/loading-stage-b36731.py --object 0x10232f90 --set 7

`--set` writes the byte, which is a blunt instrument: it makes the *screen* say a stage without
running whatever the transition would have run. It answers one question only, and answers it
well: does the client proceed when told it has advanced, or is it genuinely blocked on work?

Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import subprocess
import sys
import time

NAMES = {
    0: "LOAD_GEO",
    1: "STARTUP",
    2: "CONFIG_GRAPHICS",
    3: "WAIT_FOR_SERVER",
    4: "CONNECT_TO_SERVER",
    5: "LOAD_GAME_OBJECTS",
    6: "LOAD_EDITOR",
    7: "DOWNLOAD_WORLD",
    8: "POPULATE_WORLD",
    9: "TELEPORTING",
    10: "EDITOR",
    11: "VIEWER",
    12: "LEAVING_WORLD",
    13: "READY_TO_PLAY",
    14: "CHARACTER_CREATION",
}

STAGE_OFFSET = 0xD34

parser = argparse.ArgumentParser()
parser.add_argument("--object", required=True, help="loading-screen object address, e.g. 0x10232f90")
parser.add_argument("--set", type=int, default=None, help="force the current stage (display only)")
parser.add_argument(
    "--request",
    type=int,
    default=None,
    help="request a transition properly, so the client runs it",
)
parser.add_argument("--watch", type=float, default=0, help="poll for this many seconds")
args = parser.parse_args()

# Bracketed first character: pgrep -f otherwise matches this script's own command line.
pids = subprocess.run(
    ["pgrep", "-f", "[S]kySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running")

pid = int(pids[0])
addr = int(args.object, 0) + STAGE_OFFSET


def read():
    with open(f"/proc/{pid}/mem", "rb", 0) as mem:
        mem.seek(addr)
        return mem.read(1)[0]


def describe(value):
    return f"{value} ({NAMES.get(value, 'unknown')})"


print(f"pid {pid}, stage byte at {addr:#010x}")

if args.request is not None:
    # A transition is *requested*, not assigned. The per-frame tick (FUN_00714cf0) checks the
    # pending flag at +0xD36, and if set, applies the stage at +0xD35 through FUN_00718d00,
    # which is what actually writes +0xD34 and runs whatever the new stage does.
    #
    # Writing +0xD34 directly, as `--set` does, only changes what the screen says.
    before = read()

    with open(f"/proc/{pid}/mem", "r+b", 0) as mem:
        mem.seek(addr + 1)  # +0xD35, the requested stage
        mem.write(bytes([args.request]))
        mem.seek(addr + 2)  # +0xD36, the pending flag
        mem.write(bytes([1]))

    time.sleep(0.5)

    print(f"  requested {describe(args.request)}")
    print(f"  {describe(before)} -> {describe(read())}")
elif args.set is not None:
    before = read()

    with open(f"/proc/{pid}/mem", "r+b", 0) as mem:
        mem.seek(addr)
        mem.write(bytes([args.set]))

    print(f"  {describe(before)} -> {describe(read())} (display only)")
else:
    print(f"  {describe(read())}")

if args.watch:
    last = read()
    deadline = time.time() + args.watch

    while time.time() < deadline:
        current = read()

        if current != last:
            print(f"  {time.strftime('%H:%M:%S')}  {describe(last)} -> {describe(current)}")
            last = current

        time.sleep(0.2)

    print(f"  settled at {describe(last)}")
