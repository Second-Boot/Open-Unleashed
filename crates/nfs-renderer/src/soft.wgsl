// ENHANCEMENT E12 (soft particles): the 3D pass's depth as a single-sample `R32Float` image the
// particle shader reads. A multisampled depth cannot be sampled like a texture, so this pass takes
// the nearest of the samples per pixel (the conservative choice: a particle fades where ANY sample
// of the pixel has geometry close behind it); a single-sample depth is copied as it is. The stored
// value is the raw depth (0..1); `shader.wgsl` turns it into a view distance with the pass's planes.
@group(0) @binding(0) var depth_ms: texture_depth_multisampled_2d;
@group(0) @binding(1) var depth_1: texture_depth_2d;

@fragment
fn fs_ms(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    var d = 1.0;
    let n = i32(textureNumSamples(depth_ms));
    for (var s = 0; s < n; s++) {
        d = min(d, textureLoad(depth_ms, p, s));
    }
    return vec4<f32>(d, 0.0, 0.0, 1.0);
}

@fragment
fn fs_1(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(depth_1, vec2<i32>(pos.xy), 0), 0.0, 0.0, 1.0);
}
