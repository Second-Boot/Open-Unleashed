# hd-textures: x4 neural upscaling of the original textures

`upscale.py` upscales a directory of PNGs (as written by `asset-extractor fsh` or the game-side exporter)
by exactly x4, deterministically and resumably, driving the models directly through
[spandrel](https://github.com/chaiNNer-org/spandrel) (no ComfyUI server, headless, MPS / CUDA / CPU).

## Install

Either reuse an existing ComfyUI venv (has torch + spandrel + opencv + scipy + pillow):

    PY=~/ComfyUI-Installs/ComfyUI/ComfyUI/.venv/bin/python

or make a fresh one: `python3 -m venv .venv && .venv/bin/pip install -r requirements.txt`.

Models (about 300 MB for the three that are used; `--all` adds the bake-off candidates, about 1 GB):

    ./fetch_models.sh            # -> $NFS_UPSCALE_MODELS, default ~/ComfyUI-Shared/models/upscale_models

## Run

    $PY upscale.py IN_DIR OUT_DIR [--manifest manifest.tsv] [--fast] [--model NAME] [--force] [--limit N]

* Output keeps names and sub-directories; same-size x4 RGBA PNG. Re-running skips finished files (atomic
  writes, so an interrupted run is safe); a different configuration into the same dir is refused (`--force`).
* Deterministic: fp32, no sampling; two runs on the same machine are byte-identical (checked). MPS vs CPU may
  differ in the last bit.
* `--fast` swaps pbrify_v4 for its RealPLKSR sibling (about 3x quicker, slightly less detail).
* `OUT_DIR/review.tsv`: textures that look like text / logos / gauges (sorted by score) or belong to the classes
  `sign_text`, `text`, `logo`, `car_badge`. None of the networks reconstructs lettering faithfully; hand-check or
  redraw these (or keep bicubic).

### Manifest (optional, TSV with header)

    file<TAB>tile<TAB>class<TAB>text

`file` = path relative to IN_DIR; `tile` = 1 if the texture repeats (wrap-around context so seams stay
seamless); `class` = one of the names in `ORGANIC` / `MANMADE` / `sky` / `smooth` in `upscale.py`; `text` = 1.
Every column is optional; missing values are auto-detected (tileable = opaque with matching opposite edges,
smooth = flat/gradient -> bicubic, text = strong-edge heuristic). The final key scheme comes from the exporter;
only the `file` column has to match it.

## What it does per texture

1. Alpha: kind detected (none / 1-bit / soft). Colour under invisible texels is replaced by the nearest visible
   colour (no purple / black bleeding into the edges). RGB and alpha are upscaled separately; the alpha goes
   through RealPLKSR as greyscale (sharper and consistent with the detail, bake-off) and 1-bit alpha is re-thresholded
   to exactly 0 / 255.
2. Context padding of 8 source px (wrap if tileable, edge otherwise), rounded up to a multiple of 16, cropped after.
3. Model by class (see below).
4. Colour back-projection: the 4x-box-downsampled result is pulled back onto the source (low-frequency error
   added back). Mean colour shift 0.06 / 255 (was 0.6-3.3), RMSE vs source 1.5 (was 4.8-8.8), no loss of detail.
5. MPS returns NaN for some RealPLKSR input shapes; the code detects it and redoes that image on the CPU.

## Recommendation (bake-off 2026-10-04, 40 samples from alps + car 993, M1 Pro / MPS)

| class | model | licence |
|---|---|---|
| ground, road, rock, grass, snow, wood, foliage (alpha) | `4x-PBRify_UpscalerV4` (DAT2) | CC0 |
| car, signs, buildings, fences, text-like | `4xTextures_GTAV_rgt-s` (RGT-S) | CC-BY-4.0 (credit Philip Hofmann) |
| sky blobs, flat fills, clouds (`sky`) | bicubic | - |
| horizon mountain strip (`sky_detail`) | `4x-PBRify_UpscalerV4` (DAT2) | CC0 |
| alpha channel, `--fast` | `4x-PBRify_RPLKSRd_V3` (RealPLKSR) | CC0 |

Not used: UltraSharp and Remacri (both CC-BY-NC-SA-4.0) are non-commercial - baseline only, never ship their output.
Nomos2 / NomosUni / RealWebPhoto (photo models) amplify the 16-bit dithering into crunchy noise; SwinIR looks painted.

Time (277 textures of alps + 993 + sky, default settings): 1.5 s/texture, 7300 textures about 3-4 h; `--fast` 0.6 s/texture
(about 1.3 h).

## After the upscale (ENH-HDTEX2)

The x4 PNG pack is turned into what the game ships by `hd-pack` (`tools/hd-pack`, Rust): `hd-pack bc7` packs it into KTX2 / BC7
(`hd_textures/x4` beside the game).
The sky textures: `asset-extractor hd-export <dir> --all --only sky,sky_detail`, manifest
columns file / tile / class / text with class `sky_detail` (PBRify) or `sky` (bicubic).
