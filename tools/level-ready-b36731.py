#!/usr/bin/env python3
"""Read the ModeLevelBase fields that gate DOWNLOAD_WORLD.

The chain, all read out of the client:

    FUN_0080d320   requests DOWNLOAD_WORLD only when  [[this+0x38]+0xA4] == 5
    FUN_004d7750   advances +0xA4 through 0..5; state 4 is what sets 5
    state 2's tick calls vtable+0x10C first and does nothing if it returns false
    FUN_007a19d0   that predicate returns the byte at  this+0x51A4

So the client is stuck because +0xA4 is 2 and the readiness byte at +0x51A4 is what would let
it move on. This prints both.

ModeLevelBase has been at 0x1301e520 in every run observed so far (Wine's allocations are
deterministic), but pass --object if that changes.

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import subprocess
import sys

STATE_OFFSET = 0xA4
READY_OFFSET = 0x51A4

parser = argparse.ArgumentParser()
parser.add_argument("--object", default="0x1301e520", help="the ModeLevelBase address")
args = parser.parse_args()

pids = subprocess.run(
    ["pgrep", "-f", "[S]kySaga.exe username"], capture_output=True, text=True
).stdout.split()

if not pids:
    sys.exit("no b36731 client running")

pid = int(pids[0])
base = int(args.object, 0)
mem = open(f"/proc/{pid}/mem", "rb", 0)


def read(addr, size=1):
    mem.seek(addr)
    return mem.read(size)


state = int.from_bytes(read(base + STATE_OFFSET, 4), "little")
ready = read(base + READY_OFFSET)[0]

print(f"pid {pid}, ModeLevelBase {base:#010x}")
print(f"  +0x00a4 state = {state}   (needs 5 for DOWNLOAD_WORLD to be requested)")
print(f"  +0x51a4 ready = {ready}   (state 2's tick does nothing while this is 0)")
print(f"  bytes around +0x51a0: {read(base + 0x51A0, 16).hex(' ')}")

mem.close()
