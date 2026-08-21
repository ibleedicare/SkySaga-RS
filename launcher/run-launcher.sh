#!/usr/bin/env bash
# Run the launcher with the two things it needs on NixOS.
#
# 1. The window. eframe dlopens libwayland-client, libxkbcommon and libGL at runtime, and
#    none of them are on the default library path here, so it fails with NoWaylandLib.
#
# 2. The 32-bit Wine. The launcher spawns the client, and the client inherits this
#    environment; the system Wine is wow64-only and refuses WINEARCH=win32 outright. Running
#    the launcher inside the project's dev shell is what puts the right Wine in front of it.
#
# Both are environment, not code, which is why this is a script rather than something the
# launcher knows about.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
flake="$repo/nix"

if [ ! -f "$flake/flake.nix" ]; then
    # The tree was reorganised once already; say which assumption broke rather than failing
    # inside nix with something less obvious.
    echo "run-launcher: no flake at $flake/flake.nix" >&2
    exit 1
fi

echo "run-launcher: resolving the GUI libraries" >&2

# Cached after the first run, so this is fast.
gui_libs="$(
    nix build --no-link --print-out-paths \
        nixpkgs#wayland nixpkgs#libxkbcommon nixpkgs#libGL \
        | grep -v -- '-man$' \
        | sed 's|$|/lib|' \
        | paste -sd:
)"

export LD_LIBRARY_PATH="$gui_libs:/run/opengl-driver/lib:${LD_LIBRARY_PATH:-}"

# Where the launcher reads its account list from. The server's default is a file in its own
# working directory, so point at that rather than at wherever this was run from.
export SKYSAGA_DATABASE_URL="${SKYSAGA_DATABASE_URL:-sqlite://$repo/rust-server-port/skysaga.db}"

# The Wine prefix, explicitly.
#
# The dev shell defaults it to "$PWD/wine-prefix", so entering the shell from anywhere other
# than the repository root silently creates a *fresh, empty* prefix and the client fails with
# a wall of ole/setupapi errors from Wine bootstrapping it. The game is installed in the
# repository's prefix; say so rather than depending on which directory this was run from.
export WINEPREFIX="${WINEPREFIX:-$repo/wine-prefix}"

if [ ! -d "$WINEPREFIX" ]; then
    echo "run-launcher: no Wine prefix at $WINEPREFIX" >&2
    echo "run-launcher: set WINEPREFIX to the one holding the game" >&2
    exit 1
fi

echo "run-launcher: building" >&2
(cd "$here" && cargo build --release)

echo "run-launcher: starting (prefix $WINEPREFIX)" >&2

# From the repository root, so anything else deriving paths from $PWD agrees with the above.
cd "$repo"
exec nix develop "$flake" --command "$here/target/release/skysaga-launcher" "$@"
