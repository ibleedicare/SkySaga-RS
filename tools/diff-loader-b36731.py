#!/usr/bin/env python3
"""Diff a stalled loader against a completed one, to see what it is waiting for.

`level-pending-b36731.py` shows twelve loaders, of which `w_Trees` sits at state 1 while the
rest reach 2. They are the same kind of object, so the fields that differ are where the answer
is: a pending count, an outstanding handle, an error code.

    uv run tools/diff-loader-b36731.py --stuck 0x130d8930 --done 0x130d8830

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import re
import subprocess
import sys

SPAN = 0x80
PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{3,}")

parser = argparse.ArgumentParser()
parser.add_argument("--stuck", required=True)
parser.add_argument("--done", required=True)
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


def hint(value):
    """A short note on what a field value might be."""
    if value in PLAUSIBLE:
        found = ASCII_RUN.search(read(value, 48))

        if found:
            return "-> " + found.group().decode()[:32]

        inner = word(value)

        if inner is not None and inner in PLAUSIBLE:
            deeper = ASCII_RUN.search(read(inner, 48))

            if deeper:
                return "->-> " + deeper.group().decode()[:28]

        return "ptr"

    return ""


stuck = int(args.stuck, 0)
done = int(args.done, 0)

print(f"pid {pid}")
print(f"stuck {stuck:#010x}   done {done:#010x}\n")
print(f"{'off':>6}  {'stuck':>10}  {'done':>10}   note")

for off in range(0, SPAN, 4):
    a = word(stuck + off)
    b = word(done + off)

    if a is None or b is None:
        continue

    mark = "  <-- differs" if a != b else ""

    # Only print differing fields plus the state, or the output is mostly noise.
    if a != b or off == 0x20:
        print(f"{off:#06x}  {a:#010x}  {b:#010x}   {hint(a)}{mark}")

mem.close()
