#!/usr/bin/env bash
# Find what advances the ModeLevelBase load state, which is what gates DOWNLOAD_WORLD.
#
# `FUN_0080d320` requests the DOWNLOAD_WORLD transition only when:
#
#     *(int *)(*(int *)(this + 0x38) + 0xA4) == 5
#
# and in a stalled client that field reads 2. The object is a ModeLevelBase, allocated on the
# heap, so unlike the loading screen its address is NOT stable between runs: it has to be read
# at runtime. This breaks on the gate, takes `this` from ecx, derives the field's address, and
# watches it, all in one session.
#
# Attach EARLY. The state climbs 0 -> 1 -> 2 within seconds of the level starting to load, and
# those writes are the ones that name the code doing the advancing.
#
#   ./scripts/watch-level-state.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/logs/level-state-watch.log"

mkdir -p "$root/logs"

existing="$(pgrep -f '[S]kySaga.exe username' | head -1 || true)"

if [ -n "$existing" ]; then
    echo "a client is running (pid $existing); waiting for it to exit" >&2

    while pgrep -f '[S]kySaga.exe username' >/dev/null 2>&1; do
        sleep 1
    done
fi

echo "waiting for a new client..." >&2

pid=""
for _ in $(seq 1 600); do
    pid="$(pgrep -f '[S]kySaga.exe username' | head -1 || true)"
    [ -n "$pid" ] && break
    sleep 1
done

[ -n "$pid" ] || { echo "no client appeared" >&2; exit 1; }

echo "client is pid $pid; attaching" >&2

script="$(mktemp)"
cat > "$script" <<'EOF'
set pagination off
set confirm off
handle SIGUSR1 nostop noprint pass
handle SIGUSR2 nostop noprint pass
handle SIGSEGV nostop noprint pass
# Wine raises more than the three the skill lists. SIGQUIT in particular stopped a worker
# thread here, so the `continue` returned for that instead of the breakpoint, and the script
# then read `this` out of whatever `ecx` happened to hold. Passing them through keeps the only
# reason for a stop the one we asked for.
#
# Not SIGTRAP: breakpoints are delivered as SIGTRAP, so passing it through would disable the
# very thing we are waiting for.
handle SIGQUIT nostop noprint pass
handle SIGPIPE nostop noprint pass
handle SIGCHLD nostop noprint pass
EOF

cat >> "$script" <<EOF
attach $pid
EOF

cat >> "$script" <<'EOF'
# The gate runs every frame once the connection state machine is ticking, so this fires soon
# after attach and hands us `this` in ecx.
break *0x0080d320
continue

set $mlb = *(int *)($ecx + 0x38)
printf "ModeLevelBase at %p, state currently %d\n", $mlb, *(int *)($mlb + 0xa4)

delete
watch *(int *) ($mlb + 0xa4)
commands
  silent
  printf "LEVEL STATE -> %d  (from %p)\n", *(int *)($mlb + 0xa4), $pc
  bt 8
  continue
end
continue
EOF

timeout "${SKYSAGA_TIMEOUT:-240}" nix shell nixpkgs#gdb --command \
    gdb -q -batch -x "$script" 2>&1 | tee "$out"

rm -f "$script"

echo "wrote $out" >&2
