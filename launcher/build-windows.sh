#!/usr/bin/env bash
# Cross-build the launcher as a Windows executable, so the Windows code path can be run.
#
# # Why 32-bit
#
# SkySaga.exe and PatchedLaunch.exe are both i386, and the Wine prefix is win32. A 64-bit
# launcher cannot run in that prefix and cannot reach a 32-bit child across prefixes, so the
# only build that can actually start the game under Wine is i686.
#
# On real 64-bit Windows this constraint does not apply: a 64-bit process launches a 32-bit
# one happily. x86_64-pc-windows-gnu is the right target there, and CI builds it.
#
# # Why this proves something CI does not
#
# CI proves the Windows build compiles and its tests pass. This produces a binary that can be
# run under Wine, which takes the `Platform::Windows` branch and actually spawns the client:
# the one assumption neither unit tests nor CI can check is that PatchedLaunch.exe picks up
# SKYSAGA_ARGS from the environment when started directly rather than through the shell
# script.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${SKYSAGA_WINDOWS_TARGET:-i686-pc-windows-gnu}"

case "$target" in
    i686-pc-windows-gnu)   cross=mingw32 ;;
    x86_64-pc-windows-gnu) cross=mingwW64 ;;
    *) echo "build-windows: unsupported target $target" >&2; exit 1 ;;
esac

echo "build-windows: target $target (nix pkgsCross.$cross)" >&2

# rustup rather than the nix toolchain: the nix rustc ships only the host's standard library,
# and there is no way to add a target to it.
# Rust's windows-gnu targets link `-l:libpthread.a`, which the mingw compiler package does
# not carry: mingw-w64 keeps its pthread implementation in a separate winpthreads package.
# Without this the whole thing compiles and then fails at the final link.
pthreads="$(nix build --no-link --print-out-paths "nixpkgs#pkgsCross.$cross.windows.mingw_w64_pthreads")"

echo "build-windows: pthreads from $pthreads" >&2

# Exported rather than interpolated into the inner script: nix shell passes the environment
# through, and nesting quotes three deep is how the previous attempt silently lost it.
export SKYSAGA_PTHREADS_LIB="$pthreads/lib"

# nixpkgs builds its mingw GCC with --enable-threads=mcf, so libgcc's threading primitives
# come from mcfgthread. Rust's prebuilt windows-gnu std was built against a GCC using the
# win32/posix model, so linking the two leaves 43 undefined _MCF_* and _Unwind_* symbols.
# Supplying mcfgthread is what reconciles them.
mcf="$(nix build --no-link --print-out-paths "nixpkgs#pkgsCross.$cross.windows.mcfgthreads")"
export SKYSAGA_MCF_LIB="$mcf/lib"

echo "build-windows: mcfgthread from $mcf" >&2

exec nix shell "nixpkgs#rustup" "nixpkgs#pkgsCross.$cross.stdenv.cc" --command bash -euo pipefail -c '
    target="'"$target"'"
    here="'"$here"'"

    # Outside the repository. Putting it under $here installs ~700MB of toolchain into the
    # working tree, which git add -A will happily commit and GitHub will then reject.
    export RUSTUP_HOME="${RUSTUP_HOME:-${XDG_CACHE_HOME:-$HOME/.cache}/skysaga-rustup}"
    export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

    if ! rustup toolchain list 2>/dev/null | grep -q stable; then
        echo "build-windows: installing a rustup toolchain (first run only)" >&2
        rustup toolchain install stable --profile minimal
    fi

    rustup target add "$target"

    prefix="${target%%-*}"
    case "$prefix" in
        i686)   triple=i686-w64-mingw32 ;;
        x86_64) triple=x86_64-w64-mingw32 ;;
    esac

    # cargo needs the cross linker, and libsqlite3-sys needs a C compiler for the target: it
    # builds SQLite from source rather than linking a system one.
    upper="$(echo "$target" | tr "a-z-" "A-Z_")"
    export CARGO_TARGET_${upper}_LINKER="$triple-gcc"
    export CC_${target//-/_}="$triple-gcc"
    export AR_${target//-/_}="$triple-ar"

    # libgcc_eh.a holds the unwinder (_Unwind_RaiseException, _Unwind_Resume). It sits in the
    # target directory belonging to gcc, which is not on the default linker search path here,
    # so -lgcc_eh alone finds nothing. Asked of the compiler rather than hardcoded, because
    # the path contains both the gcc version and a nix store hash.
    #
    # No apostrophes in this block: the whole inner script is single-quoted, so one would end
    # the string and silently drop everything after it, including the build itself.
    gcc_lib="$(dirname "$("$triple-gcc" -print-libgcc-file-name)")"

    export RUSTFLAGS="${RUSTFLAGS:-} -L native=$SKYSAGA_PTHREADS_LIB -L native=$SKYSAGA_MCF_LIB -L native=$gcc_lib -C link-arg=-lmcfgthread -C link-arg=-lgcc_eh"

    cd "$here"
    exec rustup run stable cargo build --release --target "$target"
'
