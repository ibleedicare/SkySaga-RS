# The emulator in a container: SLikeNet built from source, the workspace built against it,
# and a runtime image holding nothing but the binary, the library and a data directory.
#
# The image is deliberately buildable from a bare checkout -- nothing outside this repository
# is needed to produce it. The one thing it cannot contain is the game's own data
# (`Entities.json`, `geodata.json`), which belongs to the game and is not redistributable;
# that is bind-mounted at run time. See the "Docker" section of the README.
#
#   docker compose build && docker compose up

# --- 1. libRakNet.so ------------------------------------------------------------------
#
# Its own stage because it takes minutes and changes only when the pinned SLikeNet revision
# does: every later source edit reuses this layer.

FROM debian:bookworm-slim AS raknet

RUN apt-get update \
 && apt-get install -y --no-install-recommends g++ git ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY scripts/build-raknet.sh /build-raknet.sh

# /opt/raknet/lib, not a temporary path: `raknet-sys` bakes this directory into the binary's
# rpath, so it has to be where the library will live in the runtime image too.
RUN /build-raknet.sh /opt/raknet/lib

# --- 2. the workspace -----------------------------------------------------------------

FROM rust:1.90-slim-bookworm AS build

# `cc` links the final binary; `libsqlite3-sys` is vendored and compiles its own C.
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*

COPY --from=raknet /opt/raknet/lib /opt/raknet/lib
ENV SKYSAGA_RAKNET_LIB=/opt/raknet/lib

# `raknet-sys` asks for this rpath itself, but `cargo:rustc-link-arg` from a build script
# applies only to that crate's own link -- it does not reach the binary that depends on it.
# Under nix the wrapped linker adds the RUNPATH anyway, which is why this is not needed
# there; with a plain GNU ld the binary comes out with no RUNPATH and cannot find
# libRakNet.so at startup. Set it here, for the link that actually matters.
ENV RUSTFLAGS="-C link-arg=-Wl,-rpath,/opt/raknet/lib"

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Cache mounts rather than a manifest-only pre-build: this is one workspace of fourteen
# crates, and the usual "copy the manifests, build a dummy main" trick would need a stanza
# per crate and rot the first time one is added. The binary is copied out inside the same
# RUN because a cache mount is not part of the layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p skysaga-server \
 && install -Dm755 target/release/skysaga-server /out/skysaga-server

# Where the database lives, created here so the named volume inherits an ownership the
# unprivileged runtime user can write to. Docker seeds an empty volume from the image.
RUN install -d -o 65532 -g 65532 /out/state

# --- 3. runtime -----------------------------------------------------------------------
#
# distroless/base is glibc and nothing else -- no shell, no package manager, ~20 MB. It is
# enough because libRakNet.so is linked with -static-libstdc++ and SQLite is vendored into
# the binary, so the only shared dependency left is libc.

FROM gcr.io/distroless/base-debian12:nonroot AS runtime

COPY --from=raknet /opt/raknet/lib/libRakNet.so /opt/raknet/lib/libRakNet.so

# Rust's panic unwinder needs libgcc_s, which distroless/base does not carry. Copying the
# one file is 120 kB; distroless/cc, the image that has it, is 10 MB more and brings a
# libstdc++ nothing here uses.
COPY --from=build /lib/x86_64-linux-gnu/libgcc_s.so.1 /lib/x86_64-linux-gnu/libgcc_s.so.1
COPY --from=build /out/skysaga-server /usr/local/bin/skysaga-server
COPY --from=build --chown=65532:65532 /out/state /var/lib/skysaga

# The game's data, bind-mounted by compose. Declared here so a missing mount fails with the
# server's own "no such file" rather than something obscure.
ENV SKYSAGA_DATA_DIR=/data \
    SKYSAGA_DATABASE_URL=sqlite:///var/lib/skysaga/skysaga.db \
    RUST_LOG=info

#  web :5164/tcp   auth :10106/tcp   chat :4444/tcp   game :42069/udp
EXPOSE 5164/tcp 10106/tcp 4444/tcp 42069/udp

USER nonroot
ENTRYPOINT ["/usr/local/bin/skysaga-server"]
