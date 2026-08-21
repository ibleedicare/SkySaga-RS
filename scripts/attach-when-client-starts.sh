#!/usr/bin/env bash
# Wait for an Alpha V10 client to appear, then attach the live-debug harness to it.
#
# The harness can attach to a client it did not spawn (kernel.yama.ptrace_scope=0), but only
# if it knows the pid, and the pid does not exist until someone launches the game. Polling for
# it here removes the coordination problem: arm this first, launch the client whenever.
#
#   ./scripts/attach-when-client-starts.sh 0x7d129e:get_loading_stage [0xADDR:label ...]
#
# Note the bracketed first character in the pattern. `pgrep -f` matches its own caller's
# command line otherwise, which is the same self-match that makes `pkill -f` exit 144.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

[ "$#" -gt 0 ] || { echo "usage: attach-when-client-starts.sh 0xADDR[:label] ..." >&2; exit 2; }

echo "waiting for an Alpha V10 client..." >&2

pid=""
for _ in $(seq 1 600); do
    pid="$(pgrep -f '[S]kySaga.exe username' | head -1 || true)"
    [ -n "$pid" ] && break
    sleep 1
done

[ -n "$pid" ] || { echo "no client appeared within 600s" >&2; exit 1; }

# The loading stage we care about is reached a few seconds in; attaching during startup would
# only slow that down.
echo "client is pid $pid, letting it settle" >&2
sleep 20

exec env \
    SKYSAGA_BUILD=36731 \
    SKYSAGA_WINE="${SKYSAGA_WINE:-/nix/store/bls04v3mzlz0gzz4hfx89pli90f8sj5m-wine-11.0/bin/wine}" \
    SKYSAGA_TRIGGER="pid:$pid" \
    SKYSAGA_TIMEOUT="${SKYSAGA_TIMEOUT:-90}" \
    SKYSAGA_OUT="${SKYSAGA_OUT:-$root/logs/live-debug-stage}" \
    "$root/.claude/skills/skysaga-live-debug/scripts/live-debug.sh" trace "$@"
