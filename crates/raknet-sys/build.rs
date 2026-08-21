//! Locate `libRakNet.so` and tell cargo to link it.
//!
//! The library is SLikeNet built by the flake (`nix build .#raknet`); see the repository
//! README for why it has to be built rather than taken from the emulator's checked-in
//! `RakNet.dll`, which is a Windows PE.
//!
//! Search order:
//!   1. `SKYSAGA_RAKNET_LIB`  — an explicit directory
//!   2. `../.raknet/lib`      — the symlink the repo keeps to the nix store path
//!   3. `../result/lib`       — a plain `nix build` result

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=SKYSAGA_RAKNET_LIB");

    let candidates: Vec<PathBuf> = std::env::var_os("SKYSAGA_RAKNET_LIB")
        .map(|dir| vec![PathBuf::from(dir)])
        .unwrap_or_else(|| {
            let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .canonicalize()
                .expect("workspace root");

            let repo = workspace.parent().expect("repository root").to_path_buf();

            vec![repo.join(".raknet/lib"), repo.join("result/lib")]
        });

    let found = candidates
        .iter()
        .find(|dir| dir.join("libRakNet.so").exists());

    let Some(dir) = found else {
        panic!(
            "libRakNet.so not found in {}.\n\
             Build it with `./scripts/build-raknet.sh` (or `nix build .#raknet`), or point\n\
             SKYSAGA_RAKNET_LIB at the directory containing it.",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };

    let dir: &Path = dir;

    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=dylib=RakNet");

    // Bake the path in, so tests and the server binary run without LD_LIBRARY_PATH.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
    println!("cargo:rerun-if-changed={}", dir.join("libRakNet.so").display());
}
