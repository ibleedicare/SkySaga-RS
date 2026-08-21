#!/usr/bin/env python3
"""Walk the object hanging off a resource entry's +0x04, looking for anything that names it.

The stuck entry found by `resource-states-b36731.py` has no name at any offset in the entry
itself: `+0x00` is a vtable shared by every entry. `+0x04` points at a per-entry object, so that
is where an identity would live. This walks it breadth-first, following pointers a few levels
deep and reporting any printable text, so the resource that never finishes loading can be named.

    uv run tools/walk-resource-object-b36731.py --addr 0x100361f0 --depth 3

Read-only. Needs kernel.yama.ptrace_scope=0.
"""
import argparse
import re
import subprocess
import sys

PLAUSIBLE = range(0x00010000, 0xC0000000)
ASCII_RUN = re.compile(rb"[ -~]{4,}")

parser = argparse.ArgumentParser()
parser.add_argument("--addr", required=True)
parser.add_argument("--depth", type=int, default=3)
parser.add_argument("--span", type=int, default=64, help="bytes to scan at each object")
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


seen = set()
findings = []


def walk(addr, depth, path):
    if depth < 0 or addr in seen or addr not in PLAUSIBLE:
        return

    seen.add(addr)
    blob = read(addr, args.span)

    if not blob:
        return

    for match in ASCII_RUN.finditer(blob):
        text = match.group().decode()

        # Skip runs that are really just pointer bytes that happen to be printable.
        if len(text) >= 4 and sum(c.isalnum() or c in "_-./" for c in text) >= len(text) - 1:
            findings.append((path + f"+{match.start():#04x}", text))

    for off in range(0, args.span, 4):
        value = int.from_bytes(blob[off : off + 4], "little")

        if value in PLAUSIBLE:
            walk(value, depth - 1, path + f"+{off:#04x}->")


root = int(args.addr, 0)

print(f"pid {pid}, walking {root:#010x} to depth {args.depth}\n")
print("raw: " + read(root, args.span).hex(" "))
print()

walk(root, args.depth, "")

if findings:
    for path, text in findings[:40]:
        print(f"  {path:34} {text}")
else:
    print("  no text found; the identity is probably a hash rather than a string")

mem.close()
