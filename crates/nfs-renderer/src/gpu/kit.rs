//! Small helpers of the frame core: building a list on worker threads in order (`par_build`), the texel copy of one image's
//! level and a whole level chain (`level_copy`, `write_levels`), the surface-format test (`is_bgra`) and the half-float codecs
//! (`f16_to_f32`, `f32_to_f16`). Nothing here draws.

/// The most worker threads a build uses.
const MAX_WORKERS: usize = 8;

/// `items.iter().map(f).collect()`, on up to [`MAX_WORKERS`] threads (the machine's cores, at most one per item), the
/// results in the items' order. wgpu's device and queue are `Send + Sync`: textures and pipelines are made in parallel.
pub(super) fn par_build<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_WORKERS).min(items.len());
    par_build_on(items, workers, f)
}

/// [`par_build`] on `workers` threads (1 or none: on this thread).
pub(super) fn par_build_on<T: Sync, R: Send>(items: &[T], workers: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = items.chunks(items.len().div_ceil(workers)).map(|c| s.spawn(move || c.iter().map(f).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("build worker")).collect()
    })
}

/// The copy target of `level` of a 2D texture, from its origin.
pub(super) fn level_copy(texture: &wgpu::Texture, level: u32) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo { texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }
}

/// The copy target of `layer` of an array texture at mip `level` (a layer of a 2D array, or `level_copy` for layer 0 of a plain 2D one).
pub(super) fn layer_copy(texture: &wgpu::Texture, level: u32, layer: u32) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo { texture, mip_level: level, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All }
}

/// Copy a `size x size` image at mip `level` from layer `src_layer` of `src` into layer `dst_layer` of `dst`.
pub(super) fn copy_layer(encoder: &mut wgpu::CommandEncoder, src: &wgpu::Texture, src_layer: u32, dst: &wgpu::Texture, dst_layer: u32, level: u32, size: u32) {
    encoder.copy_texture_to_texture(layer_copy(src, level, src_layer), layer_copy(dst, level, dst_layer), extent(size, size));
}

/// A `width x height` extent of one layer.
pub(super) fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d { width, height, depth_or_array_layers: 1 }
}

/// Write `levels` (level 0 first) of a `width x height` texture: `layout(w, h)` gives a level of `w x h` texels its copy
/// extent, bytes per row and rows (a block-compressed level copies whole blocks).
pub(super) fn write_levels(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32, levels: &[Vec<u8>], layout: impl Fn(u32, u32) -> (wgpu::Extent3d, u32, u32)) {
    let (mut w, mut h) = (width, height);
    for (level, data) in levels.iter().enumerate() {
        let (size, bytes_per_row, rows) = layout(w, h);
        queue.write_texture(level_copy(texture, level as u32), data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(rows) }, size);
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

/// A surface format stores blue first (the read-backs swap it into RGBA).
pub(super) fn is_bgra(format: wgpu::TextureFormat) -> bool {
    matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb)
}

/// IEEE half to single (the lens probe's read of the float image).
pub(super) fn f16_to_f32(h: u16) -> f32 {
    let (sign, exp, man) = ((h >> 15) as u32, ((h >> 10) & 0x1F) as i32, (h & 0x3FF) as u32);
    let v = match exp {
        0 => man as f32 * 2f32.powi(-24),
        31 => {
            if man == 0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + man as f32 / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 { -v } else { v }
}

/// Single to IEEE half (round to nearest even; overflow to infinity, tiny values to zero / subnormals).
pub(crate) fn f32_to_f16(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - e);
        let round = (m >> 12) & 1 != 0 && ((m & 0xFFF) != 0 || (m >> 13) & 1 != 0);
        return sign | ((m >> 13) as u16 + round as u16);
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let round = (man >> 12) & 1 != 0 && ((man & 0xFFF) != 0 || (man >> 13) & 1 != 0);
    h += round as u32;
    sign | h as u16
}

#[cfg(test)]
mod tests {
    use super::f16_to_f32;

    /// ENHANCEMENT E7: the lens probe's half-float decode (IEEE binary16).
    #[test]
    fn half_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24)); // the smallest subnormal
        assert_eq!(f16_to_f32(0x0400), 2f32.powi(-14)); // the smallest normal
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95); // 1/3 rounded
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
    }
}
