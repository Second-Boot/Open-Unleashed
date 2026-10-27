//! `asset-extractor hd-export` (ENHANCEMENT E41): dump the 3D world's textures as RGBA8 PNGs named by the key the game's
//! loader computes (`nfs_assets::hdpack`), plus `manifest.tsv`, for the upscaling pipeline.
//!
//! ```text
//! asset-extractor hd-export <outdir> --track alps --car 993      # the pilot set
//! asset-extractor hd-export <outdir> --all                       # every track and every car of the install
//! options: --game <root> (else NFS5_GAME_PATH / the usual places)  --no-png (manifest only)  --only <class,class> (e.g. sky,sky_detail)
//! ```
//!
//! Output: `<outdir>/<key>.png` (key = `track/<track>/<entry>`, `car/<model>/f<N>/<entry>`, `carglobal/<file>/<entry>`,
//! `car/<model>/damage_<d|l>/<entry>`; see `crates/nfs-assets/src/hdpack.rs`) and `<outdir>/manifest.tsv`:
//!
//! `key class container entry w h format alpha wrap hd note`
//!
//! * `class`: `track`, `car`, `car-damage`, `sky`, `sky_detail` (exported, `hd=yes`); `sky` / `env` / `sprite` / `car-env` with `hd=no`
//!   (listed, no PNG: the textures whose pixel size or sampling is not a surface UV, or that the game reads on the CPU; the
//!   reason is in `note`). `sky_detail` is the horizon mountain strip (`horz`, `hor0`..: real detail, the upscaler's PBRify
//!   model), `sky` with `hd=yes` the cloud layers (flat gradients: bicubic).
//! * `alpha`: `none` (all opaque), `1bit` (0 / 255), `levels:N` (N distinct values), `smooth`; a tinted car entry says `slot`.
//! * `wrap`: the address modes of the materials that sample it (`wrap/wrap` = tiles, `clamp/clamp`, `mixed:...`, `unused`);
//!   `-` for the cars' pages (the entries are blitted into a page).
//!
//! The upscaled image must be exactly `scale x` the size in the manifest (default 4, `pack.txt` `scale=N` in the pack folder).
//! The game takes the colour from the pack and re-applies the original's alpha semantics itself (`hdpack::AlphaPolicy`).

use anyhow::{bail, Context, Result};
use nfs_assets::{car::CarAssets, hdpack, track::TrackAssets, GamePaths};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

struct Row {
    key: String,
    class: &'static str,
    container: String,
    entry: String,
    w: u32,
    h: u32,
    format: String,
    alpha: String,
    wrap: String,
    hd: bool,
    note: String,
}

struct Export {
    out: PathBuf,
    png: bool,
    rows: Vec<Row>,
    /// key -> (container, entry): a collision of two sources on one key is an error
    seen: HashMap<String, (String, String)>,
    pngs: usize,
    /// `--only <class>[,<class>]`: only rows of these classes are kept (and written)
    only: Option<Vec<String>>,
}

impl Export {
    fn add(&mut self, row: Row, rgba: Option<&[u8]>) -> Result<()> {
        if self.only.as_ref().is_some_and(|o| !o.iter().any(|c| c == row.class)) {
            return Ok(());
        }
        if let Some((c, e)) = self.seen.get(&row.key) {
            if *c != row.container || *e != row.entry {
                // two HD rows on one key would make the pack ambiguous: an error. Rows that are only listed (sprites and the like,
                // never in the pack: `common.fsh` has both `spik` and `SPIK`) are disambiguated with a suffix and a warning
                let other_hd = self.rows.iter().find(|r| r.key == row.key).is_some_and(|r| r.hd);
                if row.hd || other_hd {
                    bail!("key collision: {} is both {c}/{e} and {}/{}", row.key, row.container, row.entry);
                }
                eprintln!("warning: key collision of listed-only rows: {} is both {c}/{e} and {}/{}: the second is listed as {}.dup (never part of the pack)", row.key, row.container, row.entry, row.key);
                let mut row = row;
                row.key = format!("{}.dup", row.key);
                row.note = format!("{}; same key as {c}/{e} up to case", row.note);
                self.seen.insert(row.key.clone(), (row.container.clone(), row.entry.clone()));
                self.rows.push(row);
                return Ok(());
            }
            // the same entry name twice in one container (or a shared pack met again): the loader keeps the LAST
            // image under a name (`tex_ids.insert`), so does the export — the manifest row is replaced
            if let Some(old) = self.rows.iter_mut().find(|r| r.key == row.key) {
                if (old.w, old.h) != (row.w, row.h) {
                    eprintln!("warning: {} appears twice with different sizes ({}x{} then {}x{}): the last wins in the pack; the game applies the pack image only to the entry it fits (the other keeps the original)", row.key, old.w, old.h, row.w, row.h);
                }
                *old = row;
                if let (true, true, Some(px)) = (self.png, old.hd, rgba) {
                    let path = self.out.join(format!("{}.png", old.key));
                    image::save_buffer(&path, px, old.w, old.h, image::ColorType::Rgba8).with_context(|| path.display().to_string())?;
                }
                return Ok(());
            }
        }
        self.seen.insert(row.key.clone(), (row.container.clone(), row.entry.clone()));
        if let (true, true, Some(px)) = (self.png, row.hd, rgba) {
            let path = self.out.join(format!("{}.png", row.key));
            std::fs::create_dir_all(path.parent().unwrap())?;
            image::save_buffer(&path, px, row.w, row.h, image::ColorType::Rgba8).with_context(|| path.display().to_string())?;
            self.pngs += 1;
        }
        self.rows.push(row);
        Ok(())
    }
}

fn alpha_kind(rgba: &[u8], tinted: bool) -> String {
    if tinted {
        return "slot".into();
    }
    match hdpack::AlphaPolicy::of(rgba) {
        hdpack::AlphaPolicy::Opaque => "none".into(),
        hdpack::AlphaPolicy::OneBit => "1bit".into(),
        hdpack::AlphaPolicy::Levels(v) => format!("levels:{}", v.len()),
        hdpack::AlphaPolicy::Smooth => "smooth".into(),
    }
}

fn mode(a: u32) -> &'static str {
    match a {
        0 => "clamp",
        1 => "wrap",
        2 => "mirror",
        _ => "?",
    }
}

fn fmt_name(code: u8) -> String {
    match nfs_formats::fsh::PixelFormat::from_code(code & 0x7F) {
        Some(f) => format!("{f:?}"),
        None => format!("0x{:02X}", code & 0x7F),
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let mut out: Option<PathBuf> = None;
    let (mut tracks, mut cars) = (Vec::<String>::new(), Vec::<String>::new());
    let (mut all, mut png, mut game) = (false, true, None::<PathBuf>);
    let mut only: Option<Vec<String>> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--track" => tracks.push(it.next().context("--track needs a name")?.to_lowercase()),
            "--car" => cars.push(it.next().context("--car needs a model")?.clone()),
            "--all" => all = true,
            "--no-png" => png = false,
            "--only" => only = Some(it.next().context("--only needs a class list")?.split(',').map(|c| c.trim().to_string()).collect()),
            "--game" => game = Some(PathBuf::from(it.next().context("--game needs a path")?)),
            o if o.starts_with("--") => bail!("unknown option {o}"),
            o => out = Some(PathBuf::from(o)),
        }
    }
    let Some(out) = out else { bail!("usage: asset-extractor hd-export <outdir> [--track <name>]... [--car <model>]... [--all] [--game <root>] [--no-png] [--only <class,class>]") };
    let paths = match game {
        Some(root) => GamePaths { root },
        None => GamePaths::locate()?,
    };
    if all {
        tracks = list_stems(&paths.track_dir(), "crp");
        cars = list_stems(&paths.car_model_dir(), "crp");
    }
    if tracks.is_empty() && cars.is_empty() {
        bail!("nothing to export: give --track / --car or --all");
    }
    std::fs::create_dir_all(&out)?;
    let mut ex = Export { out: out.clone(), png, rows: Vec::new(), seen: HashMap::new(), pngs: 0, only };
    for t in &tracks {
        export_track(&paths, t, &mut ex).with_context(|| format!("track {t}"))?;
    }
    // `--only` without a car class: the cars are not walked at all
    let cars_wanted = ex.only.as_ref().is_none_or(|o| o.iter().any(|c| c.starts_with("car")));
    for c in cars.iter().filter(|_| cars_wanted) {
        export_car(&paths, c, &mut ex).with_context(|| format!("car {c}"))?;
    }
    if all {
        export_listed_only(&paths, &mut ex)?;
    }
    let mut m = String::from("key\tclass\tcontainer\tentry\tw\th\tformat\talpha\twrap\thd\tnote\n");
    for r in &ex.rows {
        writeln!(m, "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", r.key, r.class, r.container, r.entry, r.w, r.h, r.format, r.alpha, r.wrap, if r.hd { "yes" } else { "no" }, r.note)?;
    }
    std::fs::write(out.join("manifest.tsv"), m)?;
    let hd = ex.rows.iter().filter(|r| r.hd).count();
    println!("{} PNGs written to {} ({} manifest rows, {} for HD, {} listed only)", ex.pngs, out.display(), ex.rows.len(), hd, ex.rows.len() - hd);
    Ok(())
}

/// The file stems of `dir`'s `*.<ext>` files (case-insensitive), sorted, lower-case.
fn list_stems(dir: &Path, ext: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    v.sort();
    v.dedup_by_key(|s| s.to_lowercase());
    v
}

fn export_track(paths: &GamePaths, name: &str, ex: &mut Export) -> Result<()> {
    let t = TrackAssets::load(paths, name)?;
    let mats = t.materials()?;
    let projectors: std::collections::HashSet<String> = nfs_game::world::track_projectors(&t).iter().map(|p| hdpack::safe(&p.texture)).collect();
    let mut n = 0;
    for tex in &t.textures {
        // the address modes of every material that samples it
        let mut modes: Vec<String> = mats.iter().filter(|m| m.texture_name() == tex.name).map(|m| format!("{}/{}", mode(m.address[0]), mode(m.address[1]))).collect();
        modes.sort();
        modes.dedup();
        let wrap = match modes.len() {
            0 => "unused".to_string(),
            1 => modes[0].clone(),
            _ => format!("mixed:{}", modes.join("|")),
        };
        let excluded = projectors.contains(&hdpack::safe(&tex.name));
        let rgba = &tex.mips[0];
        ex.add(
            Row {
                key: hdpack::key_track(name, &tex.name),
                class: if excluded { "sprite" } else { "track" },
                container: format!("Track/{name}.fsh"),
                entry: tex.name.trim().to_string(),
                w: tex.width,
                h: tex.height,
                format: fmt_name(tex.code),
                alpha: alpha_kind(rgba, false),
                wrap,
                hd: !excluded,
                note: if excluded { "excluded: the car-shadow projector's map (projective sampling, not a surface UV)".into() } else { String::new() },
            },
            Some(rgba),
        )?;
        n += 1;
    }
    // listed only: the track's `.env` (reflection / sky maps, a sphere-mapped lookup, not a surface texture)
    for tex in &t.env_textures {
        ex.add(
            Row { key: format!("env/{}/{}", hdpack::safe(name), hdpack::safe(&tex.name)), class: "env", container: format!("Track/{name}.env"), entry: tex.name.trim().to_string(), w: tex.width, h: tex.height, format: fmt_name(tex.code), alpha: alpha_kind(&tex.mips[0], false), wrap: "-".into(), hd: false, note: "excluded: reflection / sky-map lookups (spherical UV, 3D-world but low-detail by design)".into() },
            None,
        )?;
    }
    // the track's sky pack (`Track/Sky/<track>.fsh`, `sky.rs`): the horizon mountain strip and the cloud layers are GPU textures
    // sampled by NORMALIZED UVs (the ring's tile fractions `segment_uv`, the cloud grid's `j / a`, `i / b`), so their size does not
    // matter and they take the pack; every other entry is a colour map the game reads on the CPU (16 x 16 sky maps = the layers'
    // vertex colours, `hzcl` = the ring's 16 x 2 tint strip, `ltmp` = the lightning's colours)
    let sky_file = nfs_formats::sky::SkyFile::parse(&t.sky_params.join("\n")).ok();
    let mut clouds: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in sky_file.iter().flat_map(|f| &f.blocks) {
        clouds.extend(b.clouds.iter().flatten().map(|c| c.trim().to_lowercase()));
        clouds.extend(b.regions.iter().filter_map(|r| r.cloud.as_ref()).map(|c| c.trim().to_lowercase()));
    }
    for tex in &t.sky_textures {
        let n = tex.name.trim().to_lowercase();
        let horizon = n == "horz" || (n.len() == 4 && n.starts_with("hor") && n.as_bytes()[3].is_ascii_digit());
        let cloud = clouds.contains(&n) && n != "none" && !n.starts_with("flat");
        let (class, hd, note) = if horizon {
            ("sky_detail", true, "the horizon mountain strip: normalized tile UVs, real detail".to_string())
        } else if cloud {
            ("sky", true, "a cloud layer: normalized, repeating UVs; a flat gradient (bicubic is enough)".to_string())
        } else {
            ("sky", false, "excluded: a colour map the game reads on the CPU (the layers' vertex colours / the ring's tint / the lightning's)".to_string())
        };
        ex.add(
            Row { key: hdpack::key_sky(name, &tex.name), class, container: format!("Track/Sky/{name}.fsh"), entry: tex.name.trim().to_string(), w: tex.width, h: tex.height, format: fmt_name(tex.code), alpha: alpha_kind(&tex.mips[0], false), wrap: if hd { "wrap/wrap".into() } else { "-".into() }, hd, note },
            Some(&tex.mips[0]),
        )?;
    }
    println!("track {name}: {n} textures");
    Ok(())
}

fn export_car(paths: &GamePaths, model: &str, ex: &mut Export) -> Result<()> {
    let car = match CarAssets::load(paths, model) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("car {model}: skipped ({e})");
            return Ok(());
        }
    };
    let mut n = 0;
    for e in car.car_entries(paths)? {
        let mut note = String::new();
        if e.tinted {
            note.push_str("paint-slot alpha: the game re-applies the original's alpha values; upscale the colour, never trust the alpha; ");
        }
        if e.has_text {
            note.push_str("excluded: carries page text (plate / race number, drawn at the original size)");
        }
        if e.page_bpp == 565 && e.tinted && e.format == "Indexed8" {
            note.push_str("excluded: tinted palette entry on a 565 page (the original writes nothing)");
        }
        let hd = !e.has_text && !(e.page_bpp == 565 && e.tinted && e.format == "Indexed8");
        ex.add(Row { key: e.key.clone(), class: "car",container: e.container.clone(), entry: e.entry.clone(), w: e.width, h: e.height, format: e.format.clone(), alpha: alpha_kind(&e.rgba, e.tinted), wrap: "-".into(), hd, note: note.trim_end_matches("; ").to_string() }, Some(&e.rgba))?;
        n += 1;
    }
    // the damage decals of `<model>D.fsh` (dark paint) and `<model>L.fsh` (light paint)
    for light in [false, true] {
        for p in nfs_assets::car::damage_patches(paths, model, light)? {
            let img = &p.image;
            ex.add(
                Row {
                    key: hdpack::key_car_damage(model, light, p.index, &img.name),
                    class: "car-damage",
                    container: format!("CarModel/{model}{}.fsh", if light { "L" } else { "D" }),
                    entry: img.name.trim().to_string(),
                    w: img.width,
                    h: img.height,
                    format: fmt_name(img.code),
                    alpha: alpha_kind(&img.mips[0], false),
                    wrap: "-".into(),
                    hd: true,
                    note: format!("blended onto page {} at ({}, {}) by damage level", p.page + 1, p.x, p.y),
                },
                Some(&img.mips[0]),
            )?;
            n += 1;
        }
    }
    println!("car {model}: {n} textures");
    Ok(())
}

/// `--all`: the 3D-world packs that are not part of the pack (listed in the manifest only, with the reason).
fn export_listed_only(paths: &GamePaths, ex: &mut Export) -> Result<()> {
    for (dir, file, class, why) in [
        (paths.render_dir(), "common.fsh", "sprite", "particles, flares, sun, splashes, skid dust: sprite sheets addressed in texel / atlas units, drawn as screen / billboard sprites"),
        (paths.render_dir(), "particle.fsh", "sprite", "particle atlas: sub-rectangles in texels (particle.ini)"),
        (paths.car_model_dir(), "carcmn.fsh", "car-env", "the cars' shared environment / specular maps (spherical lookups, low-detail by design)"),
    ] {
        let Some(p) = GamePaths::find_file(&dir, file) else { continue };
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = nfs_formats::refpack::maybe_decompress(&raw) else { continue };
        let Ok(pack) = nfs_assets::texture::decode_pack(&p, &data) else { continue };
        let stem = file.trim_end_matches(".fsh");
        let folder = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        for tex in pack {
            ex.add(
                Row { key: format!("{class}/{}/{}", hdpack::safe(stem), hdpack::safe(&tex.name)), class, container: format!("{folder}/{file}"), entry: tex.name.trim().to_string(), w: tex.width, h: tex.height, format: fmt_name(tex.code), alpha: alpha_kind(&tex.mips[0], false), wrap: "-".into(), hd: false, note: format!("excluded: {why}") },
                None,
            )?;
        }
    }
    Ok(())
}
