#!/usr/bin/env python3
"""x4 neural upscaling of NFS Porsche Unleashed textures (batch, resumable, deterministic).

    upscale.py INPUT_DIR OUTPUT_DIR [--manifest manifest.tsv] [--model NAME] [--device mps|cpu|cuda]
               [--force] [--limit N] [--review review.tsv]

See README.md. Models are driven directly through spandrel (no ComfyUI server).
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import sys
import time
from pathlib import Path

import cv2
import numpy as np
from PIL import Image

# --- model registry -----------------------------------------------------------------------------
MODEL_DIR = Path(os.environ.get("NFS_UPSCALE_MODELS", "~/ComfyUI-Shared/models/upscale_models")).expanduser()

# name -> (file, licence)
MODELS = {
    "pbrify_v4": ("4x-PBRify_UpscalerV4.pth", "CC0-1.0"),
    "pbrify_rplksr": ("4x-PBRify_RPLKSRd_V3.pth", "CC0-1.0"),
    "pbrify_swinir": ("4x-PBRify_UpscalerSIR-M_V2.pth", "CC0-1.0"),
    "gtav_rgt": ("4xTextures_GTAV_rgt-s.safetensors", "CC-BY-4.0"),
    "nomos2_dat2": ("4xNomos2_hq_dat2.safetensors", "CC-BY-4.0"),
    "nomosweb_rplksr": ("4xNomosWebPhoto_RealPLKSR.safetensors", "CC-BY-4.0"),
    "realweb_dat2": ("4xRealWebPhoto_v4_dat2.safetensors", "CC-BY-4.0"),
    "nomos8k_hat": ("4xNomos8kSCHAT-S.safetensors", "CC-BY-4.0"),
    "nomosuni_dat2": ("4xNomosUniDAT2_box.safetensors", "CC-BY-4.0"),
    "ultrasharp": ("4x-UltraSharp.pth", "CC-BY-NC-SA-4.0 (NON-COMMERCIAL, baseline only)"),
    "remacri": ("4x_foolhardy_Remacri.pth", "CC-BY-NC-SA-4.0 (NON-COMMERCIAL, baseline only)"),
}
# Non-neural references (bake-off / fallback for text)
SIMPLE = {"bicubic": cv2.INTER_CUBIC, "lanczos": cv2.INTER_LANCZOS4, "nearest": cv2.INTER_NEAREST}

# class (manifest column `class`) -> model. From the bake-off (README "Recommendation"):
#   organic / ground / foliage  -> pbrify_v4  (richest natural detail, removes 16-bit dither + DXT noise)
#   hard-edged man-made (car, signs, buildings, fences, text) -> gtav_rgt (keeps edges straight, does not
#                                   invent crackle on metal / glass / paint)
#   sky (clouds) / smooth gradients -> bicubic (nothing to reconstruct; every network invents structure)
#   sky_detail (the horizon mountains) -> pbrify_v4 like terrain
ORGANIC = ["asphalt", "road", "road_marking", "rock", "grass", "snow", "foliage", "foliage_alpha", "terrain",
           "wood", "water", "ground",
           "sky_detail"]  # the horizon mountain strip (`horz`, `hor<n>`): real detail, unlike the cloud / gradient `sky`
MANMADE = ["car_badge", "car_body", "car_light", "car_interior", "car_plate", "car_wheel", "car", "sign_text",
           "text", "building", "fence", "fence_cutout", "hud", "logo"]
CLASS_MODEL: dict[str, str] = {**{c: "pbrify_v4" for c in ORGANIC}, **{c: "gtav_rgt" for c in MANMADE},
                               "sky": "bicubic", "smooth": "bicubic"}
DEFAULT_MODEL = "pbrify_v4"      # unknown / unclassified texture
TEXT_MODEL = "gtav_rgt"          # auto-detected text-like texture (also listed for review)
FAST = {"pbrify_v4": "pbrify_rplksr"}   # --fast: 5x quicker, slightly less detail
ALPHA_MODEL = "pbrify_rplksr"    # alpha channel goes through this as greyscale (bake-off: sharper, matches RGB)

SCALE = 4
PAD = 8          # source pixels of context added around every image (wrap for tiling, edge otherwise)


# --- model loading / running ----------------------------------------------------------------------
_loaded: dict[str, object] = {}


def load_model(name: str, device: str):
    if (name, device) in _loaded:
        return _loaded[(name, device)]
    import torch  # noqa: F401
    from spandrel import ModelLoader

    path = MODEL_DIR / MODELS[name][0]
    if not path.exists():
        sys.exit(f"model file missing: {path} (see README for download links)")
    d = ModelLoader().load_from_file(str(path))
    assert d.scale == SCALE, f"{name}: scale {d.scale}"
    m = d.model.to(device).eval()
    if len(_loaded) >= 2:  # keep memory bounded
        _loaded.pop(next(iter(_loaded)))
    _loaded[(name, device)] = m
    return m


def run_model(name: str, rgb: np.ndarray, device: str) -> np.ndarray:
    """rgb float32 HxWx3 in 0..1 -> float32 (4H)x(4W)x3 in 0..1."""
    import torch

    if name in SIMPLE:
        h, w = rgb.shape[:2]
        return cv2.resize(rgb, (w * SCALE, h * SCALE), interpolation=SIMPLE[name])
    xin = np.ascontiguousarray(rgb.transpose(2, 0, 1))
    for dev in ([device] if device == "cpu" else [device, "cpu"]):
        m = load_model(name, dev)
        x = torch.from_numpy(xin).unsqueeze(0).to(dev)
        with torch.no_grad():
            y = m(x).squeeze(0).float().cpu().numpy()
        if np.isfinite(y).all():  # MPS returns NaN for some RealPLKSR input shapes -> redo on the CPU
            break
    return y.clip(0, 1).transpose(1, 2, 0)


# --- single texture ----------------------------------------------------------------------------------
def alpha_kind(a: np.ndarray) -> str:
    if (a == 255).all():
        return "none"
    u = np.unique(a)
    return "binary" if len(u) <= 2 and set(u.tolist()) <= {0, 255} else "soft"


def fill_transparent(rgb: np.ndarray, valid: np.ndarray) -> np.ndarray:
    """Replace the colour of invisible texels with the nearest visible one (no colour bleed in)."""
    if valid.all() or not valid.any():
        return rgb
    from scipy.ndimage import distance_transform_edt

    idx = distance_transform_edt(~valid, return_distances=False, return_indices=True)
    return rgb[idx[0], idx[1]]


def upscale_rgba(rgba: np.ndarray, model: str, tile: bool, device: str, alpha_model: str = "bicubic",
                 backproj: float = 1.0) -> np.ndarray:
    """rgba uint8 HxWx4 -> uint8 (4H)x(4W)x4."""
    h, w = rgba.shape[:2]
    rgb = rgba[..., :3].astype(np.float32) / 255
    a = rgba[..., 3]
    kind = alpha_kind(a)
    if kind != "none":
        rgb = fill_transparent(rgb, a >= 128 if kind == "binary" else a > 0)
    # context padding; the padded size is rounded up to a multiple of 16 (window-attention archs)
    eh, ew = (-(h + 2 * PAD)) % 16, (-(w + 2 * PAD)) % 16
    pw = ((PAD, PAD + eh), (PAD, PAD + ew), (0, 0))
    mode = "wrap" if tile else "edge"
    prgb = np.pad(rgb, pw, mode=mode)
    out = run_model(model, prgb, device)
    if backproj > 0 and model not in SIMPLE:
        # colour/mood fidelity: force the 4x-box-downsampled result back onto the source
        small = cv2.resize(out, (prgb.shape[1], prgb.shape[0]), interpolation=cv2.INTER_AREA)
        diff = prgb - small
        out = out + backproj * cv2.resize(diff, (out.shape[1], out.shape[0]), interpolation=cv2.INTER_CUBIC)
    out = out.clip(0, 1)
    out = out[PAD * SCALE:(PAD + h) * SCALE, PAD * SCALE:(PAD + w) * SCALE]
    res = np.empty((h * SCALE, w * SCALE, 4), np.uint8)
    res[..., :3] = np.rint(out * 255).astype(np.uint8)
    if kind == "none":
        res[..., 3] = 255
    else:
        pa = np.pad(a.astype(np.float32) / 255, pw[:2], mode=mode)
        if alpha_model != "bicubic" and kind != "none":
            ua = run_model(alpha_model, np.repeat(pa[..., None], 3, 2), device).mean(2)
        else:
            ua = cv2.resize(pa, (pa.shape[1] * SCALE, pa.shape[0] * SCALE), interpolation=cv2.INTER_CUBIC)
        ua = ua[PAD * SCALE:(PAD + h) * SCALE, PAD * SCALE:(PAD + w) * SCALE]
        res[..., 3] = np.where(ua >= 0.5, 255, 0) if kind == "binary" else np.rint(ua.clip(0, 1) * 255)
    return res


# --- text / logo heuristic -------------------------------------------------------------------------
def _grey(rgba):
    return cv2.cvtColor(rgba[..., :3], cv2.COLOR_RGB2GRAY).astype(np.float32)


def is_smooth(rgba: np.ndarray) -> bool:
    """Flat colour / soft gradient (sky blobs, 16x16 fills): upscaling by a network only invents noise."""
    vis = rgba[..., 3] >= 128
    if not vis.any():
        return True
    return bool(np.abs(cv2.Laplacian(_grey(rgba), cv2.CV_32F))[vis].mean() < 1.5)


def looks_tileable(rgba: np.ndarray) -> bool:
    """Opaque texture whose opposite edges differ no more than neighbouring rows / columns do."""
    h, w = rgba.shape[:2]
    if min(h, w) < 16 or (rgba[..., 3] != 255).any():
        return False
    o = rgba[..., :3].astype(np.float32)
    rx = np.abs(o[:, 0] - o[:, -1]).mean() / (np.abs(o[:, 1:] - o[:, :-1]).mean() + 1e-3)
    ry = np.abs(o[0] - o[-1]).mean() / (np.abs(o[1:] - o[:-1]).mean() + 1e-3)
    return bool(rx < 2.0 and ry < 2.0)


def text_score(rgba: np.ndarray) -> float:
    """Heuristic (tuned on alps + car 993): share of very strong gradient pixels. Natural textures stay
    below ~0.14, signs / lettering / gauges / badges are 0.25-0.6. Car panels and wheels also reach 0.2+
    (harmless: they get the hard-edge model; the review list is sorted by this score, look at the top)."""
    h, w = rgba.shape[:2]
    vis = rgba[..., 3] >= 128
    if min(h, w) < 24 or vis.sum() < 200:
        return 0.0
    g = _grey(rgba)
    mag = np.hypot(cv2.Sobel(g, cv2.CV_32F, 1, 0), cv2.Sobel(g, cv2.CV_32F, 0, 1))
    return float(((mag > 200) & vis).sum() / vis.sum())


# --- batch driver --------------------------------------------------------------------------------------
def read_manifest(path: Path | None) -> dict[str, dict]:
    if not path:
        return {}
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    key = "file" if rows and "file" in rows[0] else next(iter(rows[0]))
    return {r[key]: r for r in rows}


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--manifest", type=Path, help="TSV: file<TAB>[tile<TAB>class<TAB>text<TAB>alpha]")
    ap.add_argument("--model", choices=sorted(MODELS) + sorted(SIMPLE), help="force one model for everything")
    ap.add_argument("--alpha-model", default=ALPHA_MODEL, help="model for the alpha channel, or bicubic")
    ap.add_argument("--fast", action="store_true", help="use the 5x faster RealPLKSR variant of pbrify_v4")
    ap.add_argument("--backproj", type=float, default=1.0, help="colour back-projection strength 0..1")
    ap.add_argument("--device", default=None)
    ap.add_argument("--force", action="store_true", help="redo existing outputs")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--review", type=Path, default=None, help="review list (default OUTPUT/review.tsv)")
    a = ap.parse_args()

    import torch
    dev = a.device or ("mps" if torch.backends.mps.is_available() else "cuda" if torch.cuda.is_available() else "cpu")
    torch.manual_seed(0)
    inp, out = Path(a.input), Path(a.output)
    man = read_manifest(a.manifest)
    files = sorted(p for p in inp.rglob("*.png"))
    if a.limit:
        files = files[: a.limit]
    out.mkdir(parents=True, exist_ok=True)
    sig = json.dumps({"model": a.model, "alpha": a.alpha_model, "bp": a.backproj, "cm": CLASS_MODEL, "fast": a.fast,
                      "d": DEFAULT_MODEL, "t": TEXT_MODEL, "pad": PAD}, sort_keys=True)
    sigf = out / ".upscale-config.json"
    if sigf.exists() and sigf.read_text() != sig and not a.force:
        sys.exit("output dir was produced with a different configuration; use --force or a new dir")
    sigf.write_text(sig)
    review = []
    done = skipped = 0
    t0 = time.time()
    for i, p in enumerate(files):
        rel = p.relative_to(inp).as_posix()
        dst = out / rel
        if dst.exists() and not a.force:
            skipped += 1
            continue
        m = man.get(rel, {})
        im = np.array(Image.open(p).convert("RGBA"))
        tile = (str(m["tile"]).strip().lower() in ("1", "true", "yes")) if m.get("tile", "") != "" else looks_tileable(im)
        cls = (m.get("class") or "").strip()
        score = text_score(im)
        is_text = str(m.get("text", "")).strip().lower() in ("1", "true", "yes") or score > 0.2
        if a.model:
            model = a.model
        elif cls in CLASS_MODEL:
            model = CLASS_MODEL[cls]
        elif is_smooth(im):
            model = "bicubic"
        else:
            model = TEXT_MODEL if is_text else DEFAULT_MODEL
        if a.fast:
            model = FAST.get(model, model)
        reasons = []
        if is_text:
            reasons.append("text/logo? (redraw / hand-check)")
        if cls in ("sign_text", "text", "logo", "car_badge"):
            reasons.append("class " + cls)
        res = upscale_rgba(im, model, tile, dev, a.alpha_model, a.backproj)
        dst.parent.mkdir(parents=True, exist_ok=True)
        tmp = dst.with_suffix(".tmp.png")
        Image.fromarray(res, "RGBA").save(tmp, optimize=False)
        os.replace(tmp, dst)
        done += 1
        if reasons:
            review.append((rel, model, ",".join(reasons), f"{score:.3f}"))
        if done % 25 == 0:
            print(f"[{i + 1}/{len(files)}] {done} done, {skipped} skipped, {time.time() - t0:.0f}s", flush=True)
    rv = a.review or out / "review.tsv"
    prev = []
    if rv.exists() and not a.force:
        prev = [tuple(l.split("\t")) for l in rv.read_text().splitlines()[1:]]
    seen = {r[0] for r in review}
    rows = [r for r in prev if r[0] not in seen] + review
    rows.sort(key=lambda r: (-float(r[3]), r[0]))
    rv.write_text("file\tmodel\treason\ttext_score\n" + "".join("\t".join(r) + "\n" for r in rows))
    print(f"done {done}, skipped {skipped}, review {len(rows)} -> {rv}, {time.time() - t0:.0f}s on {dev}")


if __name__ == "__main__":
    main()
