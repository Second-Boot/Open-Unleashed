//! What the three commands share: the file walk, the worker pool, atomic writes, the run's counters.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use nfs_assets::hdpack::{AlphaClass, ManifestRow};


/// Every `*.png` under `dir`, as (key = relative path without the extension, '/'-separated, path), sorted (deterministic order).
pub fn list_pngs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    fn walk(base: &Path, d: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
        for e in std::fs::read_dir(d).with_context(|| d.display().to_string())? {
            let p = e?.path();
            if p.is_dir() {
                walk(base, &p, out)?;
            } else if p.extension().is_some_and(|x| x == "png") {
                let rel = p.strip_prefix(base)?.with_extension("");
                out.push((rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/"), p));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Run `f` over `items` on `threads` workers (a shared counter: whole items, in order of claim).
pub fn par_for<T: Sync>(items: &[T], threads: usize, f: impl Fn(usize, &T) + Sync) {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1).min(items.len().max(1)) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(it) = items.get(i) else { break };
                f(i, it);
            });
        }
    });
}

/// Write atomically: a temp file beside the target, then a rename, so an interrupted run never leaves a half file that a re-run would skip.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The kind of a texture's alpha: its manifest word, else (no row, or a word this tool does not know) the image's own.
pub fn kind_of(row: Option<&ManifestRow>, rgba: &[u8]) -> AlphaClass {
    row.and_then(|r| AlphaClass::from_manifest(&r.alpha)).unwrap_or_else(|| AlphaClass::of_image(rgba))
}

/// `Image::open` of a PNG as an `anyhow` result carrying the path.
pub fn read_png(p: &Path) -> Result<(u32, u32, Vec<u8>)> {
    nfs_assets::hdpack::read_rgba_png(p).with_context(|| p.display().to_string())
}

/// `nfs_assets::hdpack::png_bytes` as an `anyhow` result.
pub fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    Ok(nfs_assets::hdpack::png_bytes(w, h, rgba)?)
}

/// The counters and the progress line of one command over a list of files.
pub struct Run {
    pub done: AtomicUsize,
    pub skipped: AtomicUsize,
    pub failed: AtomicUsize,
    pub started: Instant,
    total: usize,
}

impl Run {
    pub fn new(total: usize) -> Self {
        Self { done: AtomicUsize::new(0), skipped: AtomicUsize::new(0), failed: AtomicUsize::new(0), started: Instant::now(), total }
    }

    /// A file that was already there.
    pub fn skip(&self) {
        self.skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// The outcome of one file: counted, a progress line every 250 files, a failure printed with its key.
    pub fn report(&self, key: &str, r: Result<()>) {
        match r {
            Ok(()) => {
                let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 250 == 0 {
                    eprintln!("[{n}/{}] {:.0}s", self.total, self.started.elapsed().as_secs_f32());
                }
            }
            Err(e) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("FAILED {key}: {e:#}");
            }
        }
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn skipped(&self) -> usize {
        self.skipped.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Fails the command when a file did.
    pub fn finish(&self) -> Result<()> {
        if self.failed() > 0 {
            bail!("{} files failed", self.failed());
        }
        Ok(())
    }
}
