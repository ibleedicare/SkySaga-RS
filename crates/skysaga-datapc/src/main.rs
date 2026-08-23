//! `skysaga-extract-data`: unpack the game's JSON out of a client's `Data/data.pc`.
//!
//! The server needs `Entities.json` and `geodata.json`, which are the game's data and are not
//! in this repository. This produces them from a copy of the game the player already owns:
//!
//! ```text
//! skysaga-extract-data "SkySaga Infinite Isles/Client" data/
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use skysaga_datapc::{to_unix_line_endings, Archive};

const USAGE: &str = "\
usage: skysaga-extract-data <client-or-data.pc> [output-directory]

  <client-or-data.pc>   the client directory, its Data/ directory, or data.pc itself
  [output-directory]    where to write the JSON (default: ./data)

Unpacks the game's own data files so the server can read them. Point
SKYSAGA_DATA_DIR at the output directory, or use ./data, which is where
docker compose looks.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        println!("{USAGE}");

        return Ok(());
    }

    if args.len() > 2 {
        eprintln!("skysaga-extract-data: too many arguments\n\n{USAGE}");

        std::process::exit(2);
    }

    let archive_path = locate(Path::new(&args[0]))?;
    let out = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("data"));

    let bytes = std::fs::read(&archive_path)
        .with_context(|| format!("reading {}", archive_path.display()))?;

    let archive =
        Archive::parse(&bytes).with_context(|| format!("parsing {}", archive_path.display()))?;

    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;

    println!("{} ->", archive_path.display());

    let mut written = 0;

    for (hash, member) in archive.files() {
        let name = member.file_name(hash);
        let data = to_unix_line_endings(&member.data);
        let path = out.join(&name);

        std::fs::write(&path, &data).with_context(|| format!("writing {}", path.display()))?;

        println!("  {name:<24} {:>9} bytes", data.len());
        written += 1;
    }

    if written == 0 {
        bail!("{} holds no files", archive_path.display());
    }

    println!(
        "\n{written} files in {}. Run the server with SKYSAGA_DATA_DIR={} \
         (or leave them there for docker compose).",
        out.display(),
        out.display(),
    );

    Ok(())
}

/// Accept the archive itself, the `Data` directory, or the client directory above it, because
/// which of those a player has to hand is not worth making them think about.
fn locate(given: &Path) -> Result<PathBuf> {
    if given.is_file() {
        return Ok(given.to_path_buf());
    }

    if !given.exists() {
        bail!("{} does not exist", given.display());
    }

    let candidates = [given.join("data.pc"), given.join("Data/data.pc")];

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    bail!(
        "no data.pc under {}.\nLooked for {}.\n\
         Point this at the client's Data directory, or at data.pc itself.",
        given.display(),
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" and "),
    )
}
