//! `hd-pack`: the offline builder of the game's HD texture packs (ENHANCEMENT E41, `RE/notes/ENHANCEMENTS.md`).
//!
//! ```text
//! hd-pack x2   <x4-png-dir> <out-dir> [--manifest manifest.tsv] [--threads N]
//!     the x2 pack from the x4 one: a deterministic 2:1 Lanczos-3 downscale (no second neural run), alpha handled by the original's
//!     policy (premultiplied for soft alpha, re-thresholded for 1-bit alpha, kept as data for paint-slot / level alpha), tiling
//!     textures wrapped. Writes <out-dir>/<key>.png and pack.txt (scale=2).
//! hd-pack bc7  <png-dir> <out-dir> --manifest manifest.tsv [--threads N] [--quality fast|basic|slow]
//!     PNG pack -> KTX2 / BC7 pack (what the game ships): per image the alpha policy applied, a mip chain made by the game's own
//!     generator (track / sky: full chain; car pages' entries: level 0 only, they are blitted on the CPU), every level BC7-encoded
//!     (Intel ISPC texture compressor). Images that cannot be BC7 (a side not a multiple of 4) stay PNG. Resumable, multi-threaded.
//! hd-pack compare <png-dir> <ktx2-dir> --manifest manifest.tsv [--limit N]
//!     the quality of a BC7 pack against its PNGs (PSNR over the colour, alpha mismatches).
//! ```
//!
//! `manifest.tsv` is the exporter's (`asset-extractor hd-export <dir> --all [--no-png]`): its `alpha` column is the ORIGINAL texture's
//! alpha kind, which decides what happens to the alpha channel, and its `class` / `w` / `h` / `wrap` columns the rest.
// intel_tex_2's prebuilt ISPC objects carry no macOS platform load command: ld says so on every link (harmless, it assumes macOS)
#![allow(linker_messages)]

mod bc7;
mod common;
mod compare;
mod x2;

use anyhow::{bail, Context, Result};
use nfs_assets::hdpack::Manifest;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { bail!("{USAGE}") };
    let mut pos = Vec::new();
    let (mut manifest, mut threads, mut quality, mut limit) = (None::<PathBuf>, 0usize, "basic".to_string(), usize::MAX);
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--manifest" => manifest = Some(PathBuf::from(it.next().context("--manifest needs a path")?)),
            "--threads" => threads = it.next().context("--threads needs a number")?.parse()?,
            "--quality" => quality = it.next().context("--quality needs a name")?.clone(),
            "--limit" => limit = it.next().context("--limit needs a number")?.parse()?,
            o if o.starts_with("--") => bail!("unknown option {o}\n{USAGE}"),
            o => pos.push(PathBuf::from(o)),
        }
    }
    if threads == 0 {
        threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    }
    let manifest = match &manifest {
        Some(p) => Manifest::read(p).map_err(anyhow::Error::msg)?,
        None => Manifest::default(),
    };
    match (cmd.as_str(), pos.as_slice()) {
        ("x2", [i, o]) => x2::run(i, o, &manifest, threads),
        ("bc7", [i, o]) => {
            if manifest.rows.is_empty() {
                bail!("bc7 needs --manifest (the original's alpha kind decides the alpha channel)");
            }
            bc7::run(i, o, &manifest, threads, &quality)
        }
        ("compare", [p, k]) => compare::run(p, k, &manifest, limit),
        _ => bail!("{USAGE}"),
    }
}

const USAGE: &str = "usage: hd-pack x2 <x4-png-dir> <out-dir> [--manifest m.tsv] [--threads N]\n       hd-pack bc7 <png-dir> <out-dir> --manifest m.tsv [--threads N] [--quality fast|basic|slow]\n       hd-pack compare <png-dir> <ktx2-dir> --manifest m.tsv [--limit N]";
