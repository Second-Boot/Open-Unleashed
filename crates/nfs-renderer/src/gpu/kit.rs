//! Small helpers of the frame core: building a list on worker threads in order (`par_build`), the texel copy of one image's
//! level and a whole level chain (`level_copy`, `write_levels`). Nothing here draws.

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
