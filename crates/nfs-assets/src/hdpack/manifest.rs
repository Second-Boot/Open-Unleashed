//! The pack's side files shared by the exporter (`asset-extractor hd-export`) and the pack tool (`hd-pack`): `manifest.tsv`,
//! `pack.txt` and the PNG reader / writer. The upscaler script (`tools/hd-textures/upscale.py`) reads the same manifest by its header.

use std::collections::HashMap;
use std::path::Path;

/// The manifest's header: one tab-separated line, the columns of [`ManifestRow`] in this order.
pub const COLUMNS: [&str; 11] = ["key", "class", "container", "entry", "w", "h", "format", "alpha", "wrap", "hd", "note"];

/// One row of `manifest.tsv`: a texture the exporter found (or only listed, `hd == false`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRow {
    /// the pack key ([`super::key_track`] ...)
    pub key: String,
    /// one of [`super::class`]
    pub class: String,
    /// the file the texture comes from (`Track/alps.fsh`)
    pub container: String,
    /// the entry's name in it
    pub entry: String,
    /// the ORIGINAL's size
    pub w: u32,
    pub h: u32,
    /// the original's pixel format (`Argb8888`, ...)
    pub format: String,
    /// the original's alpha kind, [`super::AlphaClass::to_manifest`]
    pub alpha: String,
    /// the address modes of the materials that sample it (`wrap/wrap`, `clamp/clamp`, `mixed:...`, `unused`, `-`)
    pub wrap: String,
    /// whether the texture is part of the pack (`yes`) or only listed (`no`)
    pub hd: bool,
    pub note: String,
}

impl ManifestRow {
    /// The row's line (no newline): the columns of [`COLUMNS`] tab-separated, `hd` as `yes` / `no`.
    pub fn line(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", self.key, self.class, self.container, self.entry, self.w, self.h, self.format, self.alpha, self.wrap, if self.hd { "yes" } else { "no" }, self.note)
    }
}

/// The whole `manifest.tsv` text of `rows`: the header, then one line per row.
pub fn write_manifest<'a>(rows: impl IntoIterator<Item = &'a ManifestRow>) -> String {
    let mut m = COLUMNS.join("\t");
    m.push('\n');
    for r in rows {
        m.push_str(&r.line());
        m.push('\n');
    }
    m
}

/// A manifest read back, by key.
#[derive(Debug, Default)]
pub struct Manifest {
    pub rows: HashMap<String, ManifestRow>,
}

impl Manifest {
    /// Read a manifest by its header's column names: `key class w h alpha wrap hd` are required, `container entry format note`
    /// optional (empty when missing). A row with fewer fields than the required columns need is skipped.
    pub fn read(p: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        Self::parse(&text)
    }

    /// [`Self::read`] of the text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let head: Vec<&str> = lines.next().ok_or("empty manifest")?.split('\t').collect();
        let find = |n: &str| head.iter().position(|h| *h == n);
        let col = |n: &str| find(n).ok_or_else(|| format!("manifest has no `{n}` column"));
        let (ck, cc, cw, ch, ca, cwr, chd) = (col("key")?, col("class")?, col("w")?, col("h")?, col("alpha")?, col("wrap")?, col("hd")?);
        let (cco, cen, cfo, cno) = (find("container"), find("entry"), find("format"), find("note"));
        let mut rows = HashMap::new();
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() <= chd.max(ck).max(cc).max(cw).max(ch).max(ca).max(cwr) {
                continue;
            }
            let opt = |c: Option<usize>| c.and_then(|c| f.get(c)).copied().unwrap_or("").to_string();
            let num = |c: usize| f[c].parse::<u32>().map_err(|e| format!("manifest row `{}`: {e}", f[ck]));
            rows.insert(
                f[ck].to_string(),
                ManifestRow { key: f[ck].into(), class: f[cc].into(), container: opt(cco), entry: opt(cen), w: num(cw)?, h: num(ch)?, format: opt(cfo), alpha: f[ca].into(), wrap: f[cwr].into(), hd: f[chd] == "yes", note: opt(cno) },
            );
        }
        Ok(Self { rows })
    }
}

/// The `scale=N` of a pack folder's `pack.txt` (the first such line that parses; no range check, [`super::HdPack::open`] applies its own).
pub fn read_scale(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("pack.txt")).ok()?.lines().find_map(|l| l.trim().strip_prefix("scale=")?.trim().parse().ok())
}

/// The text of a `pack.txt`: `scale=<scale>` and one free line of notes (how the pack was made).
pub fn pack_txt(scale: u32, note: &str) -> String {
    format!("scale={scale}\n{note}\n")
}

/// Decode a PNG file to RGBA8: (width, height, pixels).
pub fn read_rgba_png(p: &Path) -> image::ImageResult<(u32, u32, Vec<u8>)> {
    let img = image::open(p)?.to_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

/// RGBA8 pixels as PNG file bytes.
pub fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> image::ImageResult<Vec<u8>> {
    let mut out = Vec::new();
    image::write_buffer_with_format(&mut std::io::Cursor::new(&mut out), rgba, w, h, image::ColorType::Rgba8, image::ImageFormat::Png)?;
    Ok(out)
}

/// Write RGBA8 pixels as a PNG file, creating the folder.
pub fn write_rgba_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    image::save_buffer(path, rgba, w, h, image::ColorType::Rgba8).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, hd: bool, note: &str) -> ManifestRow {
        ManifestRow { key: key.into(), class: "track".into(), container: "Track/alps.fsh".into(), entry: "cl10".into(), w: 256, h: 64, format: "Argb8888".into(), alpha: "levels:3".into(), wrap: "wrap/clamp".into(), hd, note: note.into() }
    }

    /// the writer's text is the one the exporter wrote before (header and a line), and it reads back to the same rows
    #[test]
    fn a_manifest_round_trips() {
        let rows = [row("track/alps/cl10", true, ""), row("sprite/common/spik", false, "excluded: x; y")];
        let text = write_manifest(&rows);
        assert_eq!(text.lines().next(), Some("key\tclass\tcontainer\tentry\tw\th\tformat\talpha\twrap\thd\tnote"));
        assert_eq!(text.lines().nth(1), Some("track/alps/cl10\ttrack\tTrack/alps.fsh\tcl10\t256\t64\tArgb8888\tlevels:3\twrap/clamp\tyes\t"));
        let back = Manifest::parse(&text).unwrap();
        assert_eq!(back.rows.len(), 2);
        for r in &rows {
            assert_eq!(&back.rows[&r.key], r);
        }
    }

    /// the pack tool's reader: columns by name in any order, optional ones may be missing, short rows are skipped
    #[test]
    fn the_reader_takes_columns_by_name() {
        let m = Manifest::parse("hd\talpha\twrap\th\tw\tclass\tkey\nyes\tnone\t-\t8\t4\tcar\tcar/x/f1/a\nshort\n").unwrap();
        let r = &m.rows["car/x/f1/a"];
        assert_eq!((r.w, r.h, r.hd, r.alpha.as_str(), r.class.as_str(), r.note.as_str()), (4, 8, true, "none", "car", ""));
        assert_eq!(m.rows.len(), 1);
        assert!(Manifest::parse("key\tclass\n").unwrap_err().contains("`w`"));
    }

    #[test]
    fn pack_txt_and_scale() {
        let dir = std::env::temp_dir().join(format!("nfs_hdpack_scale_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_scale(&dir), None);
        std::fs::write(dir.join("pack.txt"), pack_txt(2, "source=x4, hd-pack x2")).unwrap();
        assert_eq!(read_scale(&dir), Some(2));
        assert_eq!(pack_txt(4, "format=ktx2"), "scale=4\nformat=ktx2\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn png_helpers_round_trip_and_create_the_folder() {
        let dir = std::env::temp_dir().join(format!("nfs_hdpack_png_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let px: Vec<u8> = (0..2 * 2 * 4).map(|i| (i * 13) as u8).collect();
        let p = dir.join("a/b/c.png");
        write_rgba_png(&p, 2, 2, &px).unwrap();
        assert_eq!(read_rgba_png(&p).unwrap(), (2, 2, px.clone()));
        assert_eq!(std::fs::read(&p).unwrap(), png_bytes(2, 2, &px).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
