#!/usr/bin/env bash
# Catch the instruction that writes the 2017 client's loading stage.
#
# The stage is a byte at `DAT_0143785c + 0xD34`, which under Wine lands at a stable address
# across runs: 0x10233CC4. A hardware watchpoint on it reports the writing instruction, which
# is the state machine we have failed to find by reading the binary. Decompile whatever it
# reports; the caller is what decides to advance.
#
# Attach EARLY: the client reaches LOAD_GAME_OBJECTS within about a minute of launching, and
# the write we want is the one that sets it. Waiting until it is already stuck catches nothing,
# because by then the stage never changes again.
#
#   ./scripts/watch-loading-stage.sh          # waits for a client, then watches
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
addr="${SKYSAGA_STAGE_ADDR:-0x10233CC4}"
out="$root/logs/stage-watch.log"

mkdir -p "$root/logs"

# Wait for a *fresh* client. Attaching to one that is already stuck catches nothing: the
# transitions we want to see happen during startup, and by the time it is sitting at
# LOAD_GAME_OBJECTS they have all already fired.
#
# Bracketed first character so pgrep does not match this script's own command line.
existing="$(pgrep -f '[S]kySaga.exe username' | head -1 || true)"

if [ -n "$existing" ]; then
    echo "a client is already running (pid $existing); waiting for it to exit first" >&2

    while pgrep -f '[S]kySaga.exe username' >/dev/null 2>&1; do
        sleep 1
    done
fi

echo "waiting for a new Alpha V10 client..." >&2

pid=""
for _ in $(seq 1 600); do
    pid="$(pgrep -f '[S]kySaga.exe username' | head -1 || true)"
    [ -n "$pid" ] && break
    sleep 1
done

[ -n "$pid" ] || { echo "no client appeared" >&2; exit 1; }

echo "client is pid $pid; attaching now (early, on purpose)" >&2

# Attach as soon as the process exists. The early stage requests fire within seconds of the
# window appearing, so any settling delay here loses exactly the events we came for.

script="$(mktemp)"
cat > "$script" <<EOF
set pagination off
set confirm off
handle SIGUSR1 nostop noprint pass
handle SIGUSR2 nostop noprint pass
handle SIGSEGV nostop noprint pass
attach $pid
watch *(char *) $addr
commands
  silent
  printf "STAGE WRITE from %p -> %d\n", \$pc, *(char *) $addr
  bt 6
  continue
end
continue
EOF

# The outer timeout ends the run; killing gdb here would leave the client stopped at the
# watchpoint, which looks exactly like a freeze.
timeout "${SKYSAGA_TIMEOUT:-180}" nix shell nixpkgs#gdb --command \
    gdb -q -batch -x "$script" 2>&1 | tee "$out"

rm -f "$script"

echo "wrote $out" >&2
