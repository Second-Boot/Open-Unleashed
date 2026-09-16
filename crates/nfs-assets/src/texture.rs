//! SHPI → engine textures.

use crate::{fmt_err, AssetError};
use nfs_formats::fsh::Fsh;
use std::path::Path;

/// A decoded texture with its full mip chain (level 0 first), RGBA8.
#[derive(Debug, Clone)]
pub struct TextureImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
    /// True when any decoded texel has alpha < 255.
    pub has_alpha: bool,
}

/// Decode every entry of a SHPI pack. `path` is used for diagnostics only.
pub fn decode_pack(path: &Path, data: &[u8]) -> Result<Vec<TextureImage>, AssetError> {
    let fsh = Fsh::parse(data).map_err(fmt_err(path))?;
    let mut out = Vec::with_capacity(fsh.entries.len());
    for (i, e) in fsh.entries.iter().enumerate() {
        let levels = e.blocks[0].mip_levels();
        let mut mips = Vec::with_capacity(levels as usize);
        let mut width = 0;
        let mut height = 0;
        let mut has_alpha = false;
        for level in 0..levels {
            let img = fsh.decode_mip(data, i, level).map_err(fmt_err(path))?;
            if level == 0 {
                width = img.width;
                height = img.height;
                has_alpha = img.rgba.chunks_exact(4).any(|p| p[3] < 255);
            }
            mips.push(img.rgba);
        }
        out.push(TextureImage { name: e.name.clone(), width, height, mips, has_alpha });
    }
    Ok(out)
}
