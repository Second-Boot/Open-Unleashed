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

use anyhow::{bail, Context, Result};
use intel_tex_2::{bc7, RgbaSurface};
use nfs_renderer::MipAlpha;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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
        Some(p) => Manifest::read(p)?,
        None => Manifest::default(),
    };
    match (cmd.as_str(), pos.as_slice()) {
        ("x2", [i, o]) => cmd_x2(i, o, &manifest, threads),
        ("bc7", [i, o]) => {
            if manifest.rows.is_empty() {
                bail!("bc7 needs --manifest (the original's alpha kind decides the alpha channel)");
            }
            cmd_bc7(i, o, &manifest, threads, &quality)
        }
        ("compare", [p, k]) => cmd_compare(p, k, &manifest, limit),
        _ => bail!("{USAGE}"),
    }
}

const USAGE: &str = "usage: hd-pack x2 <x4-png-dir> <out-dir> [--manifest m.tsv] [--threads N]\n       hd-pack bc7 <png-dir> <out-dir> --manifest m.tsv [--threads N] [--quality fast|basic|slow]\n       hd-pack compare <png-dir> <ktx2-dir> --manifest m.tsv [--limit N]";

// ---------------------------------------------------------------------------------------------------- manifest

#[derive(Debug, Clone)]
struct Row {
    class: String,
    w: u32,
    h: u32,
    alpha: String,
    wrap: String,
    hd: bool,
}

#[derive(Default)]
struct Manifest {
    rows: HashMap<String, Row>,
}

impl Manifest {
    fn read(p: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(p).with_context(|| p.display().to_string())?;
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().context("empty manifest")?.split('\t').collect();
        let col = |n: &str| head.iter().position(|h| *h == n).with_context(|| format!("manifest has no `{n}` column"));
        let (ck, cc, cw, ch, ca, cwr, chd) = (col("key")?, col("class")?, col("w")?, col("h")?, col("alpha")?, col("wrap")?, col("hd")?);
        let mut rows = HashMap::new();
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() <= chd.max(ck).max(cc).max(cw).max(ch).max(ca).max(cwr) {
                continue;
            }
            rows.insert(f[ck].to_string(), Row { class: f[cc].into(), w: f[cw].parse()?, h: f[ch].parse()?, alpha: f[ca].into(), wrap: f[cwr].into(), hd: f[chd] == "yes" });
        }
        Ok(Self { rows })
    }
}

/// What happens to a texture's alpha channel (the original's policy, `nfs_assets::hdpack::AlphaPolicy`, from the manifest's `alpha`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// the original is opaque: alpha 255
    Opaque,
    /// 0 / 255: thresholded at 128
    OneBit,
    /// translucent (smooth): premultiplied filtering, kept
    Soft,
    /// paint-slot selectors / few levels: alpha is DATA (the game snaps it to the original's values), filtered like a colour
    Data,
}

fn kind_of(row: Option<&Row>, rgba: &[u8]) -> Kind {
    match row.map(|r| r.alpha.as_str()) {
        Some("none") => Kind::Opaque,
        Some("1bit") => Kind::OneBit,
        Some("smooth") => Kind::Soft,
        Some(a) if a == "slot" || a.starts_with("levels") => Kind::Data,
        _ => {
            // no manifest row: from the image itself
            if rgba.chunks_exact(4).all(|p| p[3] == 255) {
                Kind::Opaque
            } else if rgba.chunks_exact(4).all(|p| p[3] == 0 || p[3] == 255) {
                Kind::OneBit
            } else {
                Kind::Soft
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------- plumbing

/// Every `*.png` under `dir`, as (key = relative path without the extension, '/'-separated, path), sorted (deterministic order).
fn list_pngs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
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
fn par_for<T: Sync>(items: &[T], threads: usize, f: impl Fn(usize, &T) + Sync) {
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
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_png(p: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let img = image::open(p).with_context(|| p.display().to_string())?.to_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::write_buffer_with_format(&mut std::io::Cursor::new(&mut out), rgba, w, h, image::ColorType::Rgba8, image::ImageFormat::Png)?;
    Ok(out)
}

fn scale_of(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("pack.txt")).ok()?.lines().find_map(|l| l.trim().strip_prefix("scale=")?.trim().parse().ok())
}

// ---------------------------------------------------------------------------------------------------- x2

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f32::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// The 12 taps of a 2:1 Lanczos-3: output texel `o` covers source texels `2o` and `2o + 1` (centre at the edge between them),
/// the kernel stretched by the scale 2; tap `k` is source texel `2o + k - 5`.
fn taps() -> [f32; 12] {
    let mut w = [0f32; 12];
    for (k, t) in w.iter_mut().enumerate() {
        let i = k as f32 - 5.0;
        *t = lanczos3((i + 0.5 - 1.0) / 2.0);
    }
    let sum: f32 = w.iter().sum();
    w.iter_mut().for_each(|t| *t /= sum);
    w
}

/// One separable 2:1 pass over `nch` interleaved planes along x (`horizontal`) or y; edges wrap or clamp.
fn pass(src: &[f32], w: usize, h: usize, nch: usize, horizontal: bool, wrap: bool, taps: &[f32; 12]) -> (Vec<f32>, usize, usize) {
    let (ow, oh) = if horizontal { (w / 2, h) } else { (w, h / 2) };
    let mut out = vec![0f32; ow * oh * nch];
    let n = if horizontal { w } else { h } as i64;
    for y in 0..oh {
        for x in 0..ow {
            let o = (y * ow + x) * nch;
            let c = if horizontal { x } else { y } as i64 * 2;
            for (k, &t) in taps.iter().enumerate() {
                let mut i = c + k as i64 - 5;
                i = if wrap { i.rem_euclid(n) } else { i.clamp(0, n - 1) };
                let s = if horizontal { (y * w + i as usize) * nch } else { (i as usize * w + x) * nch };
                for ch in 0..nch {
                    out[o + ch] += t * src[s + ch];
                }
            }
        }
    }
    (out, ow, oh)
}

/// 2:1 downscale of an RGBA8 image by the alpha kind (see [`Kind`]). Channels filtered: 0..3 "weighted" colour, 3 weight / alpha, 4..7
/// plain colour (the fallback where the weight vanishes).
fn downscale2(src: &[u8], w: usize, h: usize, kind: Kind, wrap: [bool; 2]) -> Vec<u8> {
    const N: usize = 7;
    let mut planes = vec![0f32; w * h * N];
    for (i, p) in src.chunks_exact(4).enumerate() {
        let (r, g, b, a) = (p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32 / 255.0);
        let wgt = match kind {
            Kind::Opaque => 1.0,
            Kind::OneBit => (p[3] >= 128) as u8 as f32,
            Kind::Soft => a,
            Kind::Data => 1.0,
        };
        let o = i * N;
        planes[o..o + 7].copy_from_slice(&[r * wgt, g * wgt, b * wgt, if kind == Kind::OneBit { wgt } else { a }, r, g, b]);
    }
    let t = taps();
    let (p, w1, _) = pass(&planes, w, h, N, true, wrap[0], &t);
    let (p, w2, h2) = pass(&p, w1, h, N, false, wrap[1], &t);
    let mut out = vec![0u8; w2 * h2 * 4];
    let q = |v: f32| (v + 0.5).floor().clamp(0.0, 255.0) as u8;
    for i in 0..w2 * h2 {
        let f = &p[i * N..i * N + N];
        let (rgb, a): ([f32; 3], u8) = match kind {
            Kind::Opaque | Kind::Data => ([f[4], f[5], f[6]], if kind == Kind::Opaque { 255 } else { q(f[3] * 255.0) }),
            Kind::OneBit => {
                let vis = f[3] >= 0.5;
                // the weighted colour where something visible is under the footprint, the plain one otherwise
                let c = if f[3] > 1e-3 { [f[0] / f[3], f[1] / f[3], f[2] / f[3]] } else { [f[4], f[5], f[6]] };
                (c, if vis { 255 } else { 0 })
            }
            Kind::Soft => {
                let a = f[3].clamp(0.0, 1.0);
                let c = if a > 1e-3 { [f[0] / f[3], f[1] / f[3], f[2] / f[3]] } else { [f[4], f[5], f[6]] };
                (c, q(a * 255.0))
            }
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&[q(rgb[0]), q(rgb[1]), q(rgb[2]), a]);
    }
    out
}

fn cmd_x2(inp: &Path, out: &Path, m: &Manifest, threads: usize) -> Result<()> {
    if let Some(s) = scale_of(inp)
        && s != 4
    {
        bail!("{} is a scale={s} pack: x2 is made from the x4 one", inp.display());
    }
    let files = list_pngs(inp)?;
    std::fs::create_dir_all(out)?;
    write_atomic(&out.join("pack.txt"), b"scale=2\nsource=x4 downscaled 2:1 (Lanczos-3, alpha by the original's policy), hd-pack x2\n")?;
    let (done, skipped, failed) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let started = Instant::now();
    par_for(&files, threads, |_, (key, path)| {
        let target = out.join(format!("{key}.png"));
        if target.is_file() {
            skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let r = (|| -> Result<()> {
            let (w, h, rgba) = read_png(path)?;
            if w % 2 != 0 || h % 2 != 0 {
                bail!("{key}: {w}x{h} is not even");
            }
            if let Some(row) = m.rows.get(key)
                && (w, h) != (row.w * 4, row.h * 4)
            {
                bail!("{key}: {w}x{h} is not 4x the manifest's {}x{} (not an x4 pack image)", row.w, row.h);
            }
            let row = m.rows.get(key);
            let wrap = row.map_or([false, false], |r| match r.wrap.as_str() {
                "wrap/wrap" => [true, true],
                _ => [false, false],
            });
            let small = downscale2(&rgba, w as usize, h as usize, kind_of(row, &rgba), wrap);
            write_atomic(&target, &png_bytes(w / 2, h / 2, &small)?)
        })();
        match r {
            Ok(()) => {
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 250 == 0 {
                    eprintln!("[{n}/{}] {:.0}s", files.len(), started.elapsed().as_secs_f32());
                }
            }
            Err(e) => {
                failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("FAILED {key}: {e:#}");
            }
        }
    });
    println!("x2: {} written, {} already there, {} failed, {:.1}s -> {}", done.load(Ordering::Relaxed), skipped.load(Ordering::Relaxed), failed.load(Ordering::Relaxed), started.elapsed().as_secs_f32(), out.display());
    if failed.load(Ordering::Relaxed) > 0 {
        bail!("{} files failed", failed.load(Ordering::Relaxed));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------- bc7

fn settings(quality: &str, alpha: bool) -> Result<bc7::EncodeSettings> {
    Ok(match (quality, alpha) {
        ("ultrafast", false) => bc7::opaque_ultra_fast_settings(),
        ("ultrafast", true) => bc7::alpha_ultra_fast_settings(),
        ("fast", false) => bc7::opaque_fast_settings(),
        ("fast", true) => bc7::alpha_fast_settings(),
        ("basic", false) => bc7::opaque_basic_settings(),
        ("basic", true) => bc7::alpha_basic_settings(),
        ("slow", false) => bc7::opaque_slow_settings(),
        ("slow", true) => bc7::alpha_slow_settings(),
        (q, _) => bail!("unknown quality {q} (ultrafast, fast, basic, slow)"),
    })
}

/// BC7-encode one level (`w x h` RGBA8); a side that is not a multiple of 4 is padded by replicating the edge texels.
fn encode_level(rgba: &[u8], w: u32, h: u32, s: &bc7::EncodeSettings) -> Vec<u8> {
    let (pw, ph) = (w.next_multiple_of(4), h.next_multiple_of(4));
    let padded: Vec<u8>;
    let data: &[u8] = if (pw, ph) == (w, h) {
        rgba
    } else {
        let mut v = vec![0u8; (pw * ph * 4) as usize];
        for y in 0..ph {
            for x in 0..pw {
                let (sx, sy) = (x.min(w - 1), y.min(h - 1));
                v[((y * pw + x) * 4) as usize..][..4].copy_from_slice(&rgba[((sy * w + sx) * 4) as usize..][..4]);
            }
        }
        padded = v;
        &padded
    };
    bc7::compress_blocks(s, &RgbaSurface { data, width: pw, height: ph, stride: pw * 4 })
}

/// The class's mip policy: the game's track and sky textures are sampled through a chain, the car entries are blitted into pages
/// on the CPU (level 0 only is ever read).
fn wants_chain(class: &str) -> bool {
    matches!(class, "track" | "sky" | "sky_detail")
}

#[derive(Default)]
struct Totals {
    files: AtomicUsize,
    skipped: AtomicUsize,
    kept_png: AtomicUsize,
    failed: AtomicUsize,
    png_in: AtomicUsize,
    out: AtomicUsize,
    texels: AtomicUsize,
}

fn cmd_bc7(inp: &Path, out: &Path, m: &Manifest, threads: usize, quality: &str) -> Result<()> {
    settings(quality, true)?;
    let scale = scale_of(inp).unwrap_or(4);
    let files = list_pngs(inp)?;
    std::fs::create_dir_all(out)?;
    write_atomic(&out.join("pack.txt"), format!("scale={scale}\nformat=ktx2/bc7 (png kept where a side is not a multiple of 4), hd-pack bc7 quality={quality}\n").as_bytes())?;
    let t = Totals::default();
    let started = Instant::now();
    par_for(&files, threads, |_, (key, path)| {
        let (k2, png) = (out.join(format!("{key}.ktx2")), out.join(format!("{key}.png")));
        if k2.is_file() || png.is_file() {
            t.skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let r = (|| -> Result<()> {
            let Some(row) = m.rows.get(key.as_str()) else { bail!("{key}: not in the manifest") };
            if !row.hd {
                bail!("{key}: a listed-only row (hd=no) has no place in a pack");
            }
            let (w, h, mut rgba) = read_png(path)?;
            if (w, h) != (row.w * scale, row.h * scale) {
                bail!("{key}: {w}x{h}, the game wants {}x{} (scale {scale} of the original)", row.w * scale, row.h * scale);
            }
            let kind = kind_of(Some(row), &rgba);
            match kind {
                Kind::Opaque => rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255),
                Kind::OneBit => rgba.chunks_exact_mut(4).for_each(|p| p[3] = if p[3] >= 128 { 255 } else { 0 }),
                _ => {}
            }
            t.png_in.fetch_add(std::fs::metadata(path)?.len() as usize, Ordering::Relaxed);
            if w % 4 != 0 || h % 4 != 0 {
                // a BC texture's base level must be a whole number of blocks: keep the (policy-applied) PNG
                let b = png_bytes(w, h, &rgba)?;
                t.out.fetch_add(b.len(), Ordering::Relaxed);
                t.kept_png.fetch_add(1, Ordering::Relaxed);
                return write_atomic(&png, &b);
            }
            let has_alpha = kind != Kind::Opaque;
            let s = settings(quality, has_alpha)?;
            // the chain the game would have made from the PNG: Test(10) for a texture with alpha (coverage kept), Blend otherwise;
            // the sky's layers are alpha-blended: Blend
            let levels_px: Vec<Vec<u8>> = if wants_chain(&row.class) {
                let mode = if has_alpha && row.class == "track" { MipAlpha::Test(10) } else { MipAlpha::Blend };
                std::iter::once(rgba.clone()).chain(nfs_renderer::mip_chain(&rgba, w, h, mode)).collect()
            } else {
                vec![rgba]
            };
            let (mut lw, mut lh) = (w, h);
            let mut blocks = Vec::with_capacity(levels_px.len());
            for px in &levels_px {
                blocks.push(encode_level(px, lw, lh, &s));
                t.texels.fetch_add((lw * lh) as usize, Ordering::Relaxed);
                lw = (lw / 2).max(1);
                lh = (lh / 2).max(1);
            }
            let file = nfs_assets::ktx2::write_bc7(w, h, &blocks);
            t.out.fetch_add(file.len(), Ordering::Relaxed);
            write_atomic(&k2, &file)
        })();
        match r {
            Ok(()) => {
                let n = t.files.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 250 == 0 {
                    eprintln!("[{n}/{}] {:.0}s", files.len(), started.elapsed().as_secs_f32());
                }
            }
            Err(e) => {
                t.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("FAILED {key}: {e:#}");
            }
        }
    });
    let secs = started.elapsed().as_secs_f32();
    let mib = |v: &AtomicUsize| v.load(Ordering::Relaxed) as f64 / 1048576.0;
    println!(
        "bc7: {} written ({} kept as PNG), {} already there, {} failed; PNG in {:.1} MiB -> out {:.1} MiB; {:.1} Mtexel (all levels) in {:.1}s = {:.1} Mtexel/s, {} threads -> {}",
        t.files.load(Ordering::Relaxed),
        t.kept_png.load(Ordering::Relaxed),
        t.skipped.load(Ordering::Relaxed),
        t.failed.load(Ordering::Relaxed),
        mib(&t.png_in),
        mib(&t.out),
        t.texels.load(Ordering::Relaxed) as f64 / 1e6,
        secs,
        t.texels.load(Ordering::Relaxed) as f64 / 1e6 / secs.max(1e-3) as f64,
        threads,
        out.display()
    );
    if t.failed.load(Ordering::Relaxed) > 0 {
        bail!("{} files failed", t.failed.load(Ordering::Relaxed));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------- compare

fn cmd_compare(pngs: &Path, ktx: &Path, m: &Manifest, limit: usize) -> Result<()> {
    let files: Vec<(String, PathBuf)> = list_pngs(pngs)?.into_iter().filter(|(k, _)| ktx.join(format!("{k}.ktx2")).is_file()).take(limit).collect();
    let (mut n, mut sum_psnr, mut worst) = (0usize, 0f64, (f64::MAX, String::new()));
    // per alpha kind: (textures, texels, texels that change side of the 128 test, max alpha error)
    let mut by_kind: HashMap<&str, (usize, usize, usize, i32)> = HashMap::new();
    for (key, path) in &files {
        let (w, h, mut a) = read_png(path)?;
        let row = m.rows.get(key.as_str());
        let kind = kind_of(row, &a);
        match kind {
            Kind::Opaque => a.chunks_exact_mut(4).for_each(|p| p[3] = 255),
            Kind::OneBit => a.chunks_exact_mut(4).for_each(|p| p[3] = if p[3] >= 128 { 255 } else { 0 }),
            _ => {}
        }
        let img = nfs_assets::ktx2::read_bc7(&std::fs::read(ktx.join(format!("{key}.ktx2")))?).with_context(|| key.clone())?;
        if (img.width, img.height) != (w, h) {
            bail!("{key}: size mismatch");
        }
        let b = nfs_assets::ktx2::decode_level(&img.levels[0], w, h);
        let (mut se, mut cnt) = (0f64, 0usize);
        let name = match kind {
            Kind::Opaque => "opaque",
            Kind::OneBit => "1-bit",
            Kind::Soft => "smooth",
            Kind::Data => "levels / slot",
        };
        let e = by_kind.entry(name).or_default();
        e.0 += 1;
        for (p, q) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            // colour error where the texel is visible (an invisible texel's colour is never seen)
            if p[3] >= 128 || kind == Kind::Opaque {
                for c in 0..3 {
                    let d = p[c] as f64 - q[c] as f64;
                    se += d * d;
                }
                cnt += 3;
            }
            e.1 += 1;
            e.3 = e.3.max((p[3] as i32 - q[3] as i32).abs());
            if (p[3] >= 128) != (q[3] >= 128) {
                e.2 += 1;
            }
        }
        let mse = se / cnt.max(1) as f64;
        let psnr = if mse < 1e-9 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
        sum_psnr += psnr;
        n += 1;
        if psnr < worst.0 {
            worst = (psnr, key.clone());
        }
    }
    println!("compare: {n} textures; mean PSNR {:.2} dB (colour of the visible texels); worst {:.2} dB ({})", sum_psnr / n.max(1) as f64, worst.0, worst.1);
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort();
    for (k, (t, texels, flips, maxerr)) in kinds {
        println!("  alpha {k}: {t} textures, {texels} texels, {flips} change side of the 128 test, max alpha error {maxerr}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(w * h * 4).collect()
    }

    #[test]
    fn a_flat_image_stays_flat() {
        let out = downscale2(&flat(8, 8, [10, 200, 90, 255]), 8, 8, Kind::Opaque, [false, false]);
        assert_eq!(out.len(), 4 * 4 * 4);
        assert!(out.chunks_exact(4).all(|p| p == [10, 200, 90, 255]));
    }

    #[test]
    fn taps_are_normalised_and_symmetric() {
        let t = taps();
        assert!((t.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        for k in 0..6 {
            assert!((t[k] - t[11 - k]).abs() < 1e-6);
        }
    }

    #[test]
    fn one_bit_alpha_stays_one_bit_and_colour_does_not_bleed() {
        // left half opaque red, right half transparent with a green colour under it: the visible texels stay red
        let mut src = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let p = if x < 4 { [255, 0, 0, 255] } else { [0, 255, 0, 0] };
                src[(y * 8 + x) * 4..][..4].copy_from_slice(&p);
            }
        }
        let out = downscale2(&src, 8, 8, Kind::OneBit, [false, false]);
        for p in out.chunks_exact(4) {
            assert!(p[3] == 0 || p[3] == 255);
            if p[3] == 255 {
                assert!(p[0] > 250 && p[1] < 5, "visible texel keeps its colour: {p:?}");
            }
        }
        assert_eq!(out[3], 255);
        assert_eq!(out[(3) * 4 + 3], 0);
    }

    #[test]
    fn soft_alpha_is_premultiplied() {
        // an opaque white texel next to a transparent black one: the blended texel is white at half alpha, not grey
        let mut src = vec![0u8; 2 * 2 * 4];
        for (i, p) in [[255u8, 255, 255, 255], [0, 0, 0, 0], [255, 255, 255, 255], [0, 0, 0, 0]].iter().enumerate() {
            src[i * 4..i * 4 + 4].copy_from_slice(p);
        }
        // a 2x2 image is too small for the 12-tap window to see anything but clamped edges: tile it to 8x8
        let mut big = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                big[(y * 8 + x) * 4..][..4].copy_from_slice(&src[((y % 2) * 2 + x % 2) * 4..][..4]);
            }
        }
        let out = downscale2(&big, 8, 8, Kind::Soft, [true, true]);
        let p = &out[(2 * 4 + 2) * 4..][..4];
        assert!(p[3] > 100 && p[3] < 160, "half alpha: {p:?}");
        assert!(p[0] > 250, "colour stays white: {p:?}");
    }

    #[test]
    fn bc7_round_trips_a_gradient_closely() {
        let (w, h) = (16u32, 16u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                px[((y * w + x) * 4) as usize..][..4].copy_from_slice(&[(x * 16) as u8, (y * 16) as u8, 128, 255]);
            }
        }
        let blocks = encode_level(&px, w, h, &settings("basic", false).unwrap());
        let back = nfs_assets::ktx2::decode_level(&blocks, w, h);
        let worst = px.iter().zip(&back).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 6, "worst channel error {worst}");
        assert!(back.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn a_two_by_two_level_pads_and_decodes() {
        let px = flat(2, 2, [40, 80, 120, 255]);
        let blocks = encode_level(&px, 2, 2, &settings("fast", false).unwrap());
        assert_eq!(blocks.len(), 16);
        let back = nfs_assets::ktx2::decode_level(&blocks, 2, 2);
        assert_eq!(back.len(), 16);
        assert!(back.chunks_exact(4).all(|p| (p[0] as i32 - 40).abs() <= 2 && (p[2] as i32 - 120).abs() <= 2));
    }
}
