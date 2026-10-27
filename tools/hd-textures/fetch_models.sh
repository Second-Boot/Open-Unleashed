#!/bin/bash
# Downloads the upscale models used by upscale.py into $NFS_UPSCALE_MODELS
# (default ~/ComfyUI-Shared/models/upscale_models). Only the two shipped models are required;
# pass --all to also fetch the bake-off candidates (~1 GB total).
set -e
D="${NFS_UPSCALE_MODELS:-$HOME/ComfyUI-Shared/models/upscale_models}"
mkdir -p "$D"; cd "$D"
get() { [ -f "$2" ] || { echo "fetch $2"; curl -fL -o "$2.part" "$1" && mv "$2.part" "$2"; }; }
K=https://github.com/Kim2091/Kim2091-Models/releases/download
P=https://huggingface.co/Phips
get $K/4x-PBRify_UpscalerV4/4x-PBRify_UpscalerV4.pth 4x-PBRify_UpscalerV4.pth            # CC0, 134 MB
get $P/4xTextures_GTAV_rgt-s/resolve/main/4xTextures_GTAV_rgt-s.safetensors 4xTextures_GTAV_rgt-s.safetensors  # CC-BY-4.0
get $K/4x-PBRify_RPLKSRd_V3/4x-PBRify_RPLKSRd_V3.pth 4x-PBRify_RPLKSRd_V3.pth            # CC0, 28 MB (alpha + --fast)
if [ "$1" = "--all" ]; then
  get $K/4x-PBRify_UpscalerSIR-M_V2/4x-PBRify_UpscalerSIR-M_V2.pth 4x-PBRify_UpscalerSIR-M_V2.pth
  for m in 4xNomos2_hq_dat2 4xNomosWebPhoto_RealPLKSR 4xRealWebPhoto_v4_dat2 4xNomos8kSCHAT-S 4xNomosUniDAT2_box; do
    get $P/$m/resolve/main/$m.safetensors $m.safetensors; done
  # NON-COMMERCIAL baselines (never ship their output): UltraSharp CC-BY-NC-SA-4.0, Remacri CC-BY-NC-SA-4.0
  get https://huggingface.co/Kim2091/UltraSharp/resolve/main/4x-UltraSharp.pth 4x-UltraSharp.pth
  get https://huggingface.co/FacehugmanIII/4x_foolhardy_Remacri/resolve/main/4x_foolhardy_Remacri.pth 4x_foolhardy_Remacri.pth
fi
