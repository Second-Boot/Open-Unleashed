# third_party

## wgpu-ffx, wgpu-ffx-shaders-spv

AMD FidelityFX Super Resolution 3.1 (the FSR 3 upscaler of the FidelityFX SDK 1.1.4) for wgpu, vendored for the enhancement
E37 (`RE/notes/ENHANCEMENTS.md`, `crates/nfs-renderer/src/upscale/fsr.rs`).

* Upstream: https://github.com/cwfitzgerald/wgpu-ffx, commit `6fe7064064b396b30df97341c2ca957ca1b00884` (2026-09-21, "Update Cargo.lock (#21)").
* Licence: MIT (`LICENSE.MIT`: the wgpu-ffx developers and Advanced Micro Devices, Inc.). Both crates keep it.
* `wgpu-ffx-shaders-spv` ships the SPIR-V of the FidelityFX shaders **prebuilt** (42 permutations, ~1 MB, `src/fsr3upscaler/*.spv`,
  selected by `choose_shaders`): no glslc / DXC is needed at build time. Upstream's `xtask` (the shader build) and its `shaders/`
  sources are not vendored. The shaders themselves are unchanged.

### Changes against upstream (wgpu 29 -> wgpu 30, and one bug)

1. `Cargo.toml` of both crates: standalone (no `workspace = true`), `wgpu = "30"`, `publish = false`.
2. `wgpu-ffx/src/pass.rs`:
   * **FIX of an upstream bug**: the motion-vector binding of the accumulate pass was swapped. The accumulate shader (built with
     `LOW_RESOLUTION_MOTION_VECTORS` when the context flag `DISPLAY_RESOLUTION_MOTION_VECTORS` is not set) reads its slot 2 as the
     DILATED motion vectors, which `prepare_inputs` writes already scaled to UV; upstream bound the raw input vectors
     (pixels) there for that case and the dilated ones for the display-resolution case. With render-resolution vectors every
     pixel's reprojected UV was off screen, the history was never used (a static camera still looked right: zero vectors), and
     the luma-instability path painted the frame dark and speckled as soon as the camera moved. Found on the Intel UHD
     (Mesa ANV) laptop and on lavapipe by reading the accumulate shader's intermediate values (an instrumented build); ported
     for the render-resolution case, so it also holds for `DISPLAY_RESOLUTION_MOTION_VECTORS` the other way round. To be reported upstream.
   * the passthrough module descriptor of wgpu 30 has `entry_points` (name + workgroup size per entry) instead of `num_workgroups`;
   * the passthrough path is taken only when the new context flag `FsrContextFlags::SPIRV_PASSTHROUGH` is set (and the device has
     `PASSTHROUGH_SHADERS`), not whenever the device has the feature: wgpu 30 rejects SPIR-V passthrough on every backend but
     Vulkan, and upstream's tests ask the adapter for every feature;
   * the naga path (`ShaderSource::SpirV`, every backend) strips the SPIR-V `Coherent` decoration (`strip_coherent`): naga 30's
     validator refuses it on the SPD image (`globallycoherent RWTexture2D`, "memory decorations are only valid for variables in the
     storage address space"). The passthrough path (Vulkan) keeps the decoration.
3. `wgpu-ffx/src/lib.rs`: the new flag `SPIRV_PASSTHROUGH` (bit 7); the two GPU smoke tests are `#[ignore]`d (they need a GPU:
   `cargo test -p wgpu-ffx -- --ignored`).

4. **Metal (ENH-UPSCALER, 2026-10-04): the SPD pyramid's images are `Rgba16Float`, not `Rg16Float`.** wgpu's Metal backend has no
   read-write storage for `Rg16Float` (Metal's read-write texture formats, tier 2, cover R32 / RGBA32 / RGBA16 / RGBA8 / R16 / R8
   families), so the pipelines of the luma pyramid and the shading-change pyramid failed at the bind group layout on macOS.
   * `wgpu-ffx/src/resources.rs` / `lib.rs`: the `SpdMips` texture (the six read-write mips of the SPD pass, `FsrResourceName::SpdMips`)
     is `Rgba16Float` (its size accounting `* 8` bytes).
   * `wgpu-ffx-shaders-spv`: `ffx_fsr3upscaler_luma_pyramid_pass_f00859b0.spv` and `..._shading_change_pyramid_pass_b63cd97e.spv`: the
     one `OpTypeImage` of the six `rw_spd_mip0..5` images has the SPIR-V format operand `Rg16f` (7) changed to `Rgba16f` (2) in place
     (a 4-byte patch per file; the shaders `imageLoad(...).xy` and `imageStore(.., vec4(v, 0, 0))` already work on four components, so
     nothing else changes). The GLSL equivalent in the SDK's `ffx_fsr3upscaler_callbacks_glsl.h` (lines 850-855) is `rg16f` -> `rgba16f`.
     Recompiling from GLSL with glslc 2023.2 (docker, debian bookworm) gives the same shader with another optimiser result, so the
     committed files are the original ones with the format patched, not a recompile: they stay byte-for-byte the validated SPIR-V
     (the content-hash names of `mod.rs` are unchanged). `prepare_inputs`' write-only `Rg16Float` dilated vectors are unchanged (write-only
     storage is fine on Metal).
   * Vulkan / DX12: the same values are stored, read as the same two components (the pad channels are 0); the only difference is the mip
     chain's size (8 instead of 4 bytes a texel, ~0.5 % of FSR's memory at 2560x1600, 30 MB history unchanged). No change of the picture.

Not changed: the shaders (but for the format patch above), the constants, the dispatch (including the blocking `pop` of the validation
error scope in `FsrContext::dispatch`).

Needs of the device (the game asks for them at start-up, `crates/nfs-renderer/src/gpu.rs`): 8 storage textures per shader
stage, 32 KiB of compute workgroup memory (asked, up to what the adapter has), `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`
(storage-image formats beyond the base set), `TEXTURE_FORMAT_16BIT_NORM` (the Lanczos table), `FLOAT32_FILTERABLE`, `CLEAR_TEXTURE`;
`PASSTHROUGH_SHADERS` on Vulkan. With the format patch (4) FSR 3 runs on Metal too (through naga).

The fp16 permutations (`FFX_HALF = 1`) are not used. Compiled here with glslc (accumulate, luma pyramid, prepare inputs): the accumulate
pass is the one with real half-precision code, it needs `Int16` (u16), naga 30 parses and validates it but its MSL writer panics on the
float -> 16-bit-integer casts (`write_wrapped_cast`, `unreachable!`), so it cannot run through naga (Metal / DX12); Vulkan passthrough
would accept it but could not be verified (no Vulkan GPU at hand), and one pass of nine is a small part of FSR's cost.
