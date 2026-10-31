# The enhanced version

**English** | [Русский](../ru/enhanced.md)

The enhanced version (branch `enhanced`) is the same faithful engine with optional improvements to the picture. Each
improvement has its own switch; with all of them off the game is the original, frame for frame. Gameplay, physics,
the AI and the network protocol are never changed, so players of the enhanced and the original version can race each
other.

## Turning things on and off

- **In the game:** Options → Graphics → **Enhancements** — one row per improvement, with a short explanation, and three
  presets: **default**, **light** (for weaker GPUs) and **like the original**.
- **F12** switches everything between your settings and the original, live — handy for comparing.
- From the command line: `NFS_ENH=off` starts as the original, `NFS_ENH=all` with everything on.

The settings are kept in `enhancements.json` in the saves folder; the five display rows (frame output, frame-rate limit, MSAA, anisotropic filtering, render scale) are kept in `display.json` next to it.

## What is there

| Group | Improvements |
|---|---|
| Image quality | MSAA and SMAA anti-aliasing, anisotropic filtering, render scale, upscaling (AMD FSR 3.1, MetalFX on macOS), field of view, draw distance, frame output and frame-rate limit |
| Light | HDR and tone mapping, glow, a bright sun, eye adaptation, global illumination, corner shading (AO), light from the surroundings on the cars |
| Shadows | sun shadows, soft shadows when overcast, contact shadows under the cars, headlights and street lamps that light the road, headlights in tunnels |
| Atmosphere | fog and sky, haze and headlight beams, light shafts, lightning that lights the scene, the lighthouse beam, water, wet road and rain, raindrops on the windscreen |
| Cars | clear coat on the body, rounded wheels and details |
| Camera | motion blur, depth of field in replays, lens dirt, camera shake |
| Textures | HD textures (see below) |
| Display | HDR output on HDR screens (macOS EDR) |

## Photo mode extras

Photo mode itself is part of the game (see [Controls](controls.md#photo-mode)). With **HDR and tone mapping** on, the
enhanced version adds a depth of field to it, and its pictures go through the same post effects as the screen (glow,
tone mapping and the rest), rendered at the picture's own size.

| Keyboard | Pad | Action |
|---|---|---|
| F | X | depth of field on / off |
| `1` / `2` | d-pad left / right | focus distance |
| `3` / `4` | | aperture |
| `5` | | focus on the car |

## HD textures

The HD texture pack is made from the original game's textures, so it is **not distributed**. You can build it from your
own copy with the pipeline in `tools/hd-textures` (see its README; it runs on your machine with freely licensed upscaling
models) and put the result into `game/enhanced/hd_textures` (`x2/` and/or `x4/`). Without the pack the textures setting
uses the original textures.

## Requirements

The enhancements need a reasonably modern GPU with Metal (macOS), DirectX 12 or Vulkan. On integrated graphics start
with the **light** preset. Everything off runs like the original version.
