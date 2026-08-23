#!/usr/bin/env bash
#
# Build libRakNet.so from SLikeNet source.
#
# `raknet-sys` links against SLikeNet built together with its
# `raknet_backwards_compatibility` SWIG wrapper, which exports the unmangled `CSharp_*`
# entrypoints the FFI declarations name. There is no released binary of that; it has to be
# compiled. This is the same build the nix flake performs (`nix build .#raknet`), written out
# so a checkout with nothing but a C++ compiler can produce the library -- which is what the
# Docker image does.
#
#   ./scripts/build-raknet.sh              -> <repo>/.raknet/lib/libRakNet.so
#   ./scripts/build-raknet.sh /some/where  -> /some/where/libRakNet.so
#
# Requires: git, a C++14 compiler ($CXX, default g++).
#
# The wrapper was GENERATED ON WINDOWS, where `long` is 32-bit. On Linux (LP64) it is 64-bit,
# so `Write<long>` would put 64 bits on the wire where the client expects 32, every packet
# carrying an int would come out four bytes too long, and the client would stall loading the
# world. The patch below narrows `long` to fixed-width types, leaving `long long` alone.

set -euo pipefail

# Pinned, because a different SLikeNet revision can renumber the SWIG overloads --
# `__SWIG_2` is a position in a declaration list, not a name. See ARCHITECTURE.md.
readonly SLIKENET_REPO="https://github.com/SLikeSoft/SLikeNet.git"
readonly SLIKENET_REV="d5f775d789563a2d505e2afbf99a550d990bb49e"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_dir="${1:-$repo_root/.raknet/lib}"

if [ -f "$out_dir/libRakNet.so" ]; then
    echo "build-raknet: $out_dir/libRakNet.so already exists; delete it to rebuild"
    exit 0
fi

src_dir="$(mktemp -d)"
trap 'rm -rf "$src_dir"' EXIT

echo "build-raknet: fetching SLikeNet $SLIKENET_REV"
git init --quiet "$src_dir"
git -C "$src_dir" remote add origin "$SLIKENET_REPO"
git -C "$src_dir" fetch --quiet --depth 1 origin "$SLIKENET_REV"
git -C "$src_dir" checkout --quiet FETCH_HEAD

wrapper="$src_dir/bindings/raknet_backwards_compatibility/csharp/wrapper/RakNet_wrap.cxx"

echo "build-raknet: narrowing long to fixed-width types in the SWIG wrapper"
sed -i \
    -e 's/\bunsigned long long\b/@@ULL@@/g' \
    -e 's/\blong long\b/@@LL@@/g' \
    -e 's/\bunsigned long\b/uint32_t/g' \
    -e 's/\blong\b/int32_t/g' \
    -e 's/@@ULL@@/unsigned long long/g' \
    -e 's/@@LL@@/long long/g' \
    "$wrapper"

echo "build-raknet: compiling (this takes a few minutes)"
mkdir -p "$out_dir"

# -static-libstdc++/-static-libgcc so the runtime image needs nothing but libc. The library
# is the only C++ in the container; linking its support statically is cheaper than carrying
# libstdc++ for it alone.
#
# -w because SLikeNet is warning-noisy under a modern GCC and none of it is actionable here.
(
    cd "$src_dir/Source/src"
    ${CXX:-g++} ./*.cpp "$wrapper" \
        -DRAKNET_COMPATIBILITY=1 -std=c++14 -fPIC -O2 -w -pthread \
        -static-libstdc++ -static-libgcc \
        -I../include -shared -o "$out_dir/libRakNet.so"
)

echo "build-raknet: wrote $out_dir/libRakNet.so"
