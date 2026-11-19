//! The bake's cache: directory, file format, `load_or_bake`.
use super::*;

/// The cache's directory: `NFS_GI_DIR`, else `gi/` in `NFS5_SAVE_DIR` (a scripted run), else in the
/// port's data dir (next to `enhancements.json`).
pub fn cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NFS_GI_DIR") {
        return PathBuf::from(d);
    }
    crate::config::save_root().join("gi")
}

const MAGIC: &[u8; 4] = b"NGI1";

/// The file: magic, version, the input hash, the count, then per sample 5 f32 (LE).
pub fn write(path: &Path, hash: u64, samples: &[Sample]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(20 + samples.len() * 20);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&hash.to_le_bytes());
    b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for s in samples {
        for x in [s.bounce[0], s.bounce[1], s.bounce[2], s.sky, s.sun] {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &b)?;
    std::fs::rename(&tmp, path)
}

/// The cached samples when the file is there, of this version and of this input.
pub fn read(path: &Path, hash: u64, count: usize) -> Option<Vec<Sample>> {
    let b = std::fs::read(path).ok()?;
    let u32_at = |o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().expect("4 bytes")));
    if b.get(0..4)? != MAGIC || u32_at(4)? != VERSION || u64::from_le_bytes(b.get(8..16)?.try_into().ok()?) != hash || u32_at(16)? as usize != count || b.len() != 20 + count * 20 {
        return None;
    }
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"));
    Some((0..count).map(|i| {
        let o = 20 + i * 20;
        Sample { bounce: [f(o), f(o + 4), f(o + 8)], sky: f(o + 12), sun: f(o + 16) }
    }).collect())
}

/// The track's samples: the cache when it is valid, else a bake (written to the cache). Returns the
/// samples and the bake's time in seconds (`None` = read from the cache).
pub fn load_or_bake(track: &str, input: &Input) -> (Vec<Sample>, Option<f32>) {
    let hash = input.hash();
    let path = cache_dir().join(format!("{}.bin", track.to_lowercase()));
    if let Some(s) = read(&path, hash, input.recv_pos.len()) {
        return (s, None);
    }
    let t = std::time::Instant::now();
    let s = bake(input);
    let secs = t.elapsed().as_secs_f32();
    if let Err(e) = write(&path, hash, &s) {
        tracing::warn!(path = %path.display(), error = %e, "ENHANCEMENT E32: the bake could not be cached");
    }
    (s, Some(secs))
}

