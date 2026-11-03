#!/bin/sh
# ENHANCEMENT E41: report the HD texture pack that rides along in a staged data dir (sourced by stage_data.sh and make_app.sh;
# usage: hd_pack_check.sh <dst>). The pack is `hd_textures/x4`, the output of
# `hd-pack bc7` (KTX2 / BC7 files, plus a handful of PNGs where a side is not a multiple of 4), copied by the
# caller from NFS_HD_PACK; not in git. A pack made of PNGs would ship several GB: warn.
DST=$1
HD="$DST/hd_textures"
if [ ! -d "$HD" ]; then
  echo "no hd_textures: the build ships the original textures only"
  exit 0
fi
for P in x4; do
  if [ -d "$HD/$P" ]; then
    K=$(find "$HD/$P" -name '*.ktx2' | wc -l | tr -d ' ')
    G=$(find "$HD/$P" -name '*.png' | wc -l | tr -d ' ')
    echo "HD texture pack $P included: $(du -sh "$HD/$P" | cut -f1) ($K ktx2 / BC7, $G png)"
    if [ "$G" -gt 0 ] && [ "$G" -gt $((K / 10)) ]; then
      echo "WARNING: $HD/$P holds mostly PNGs - convert it with: hd-pack bc7 <png pack> <out> --manifest manifest.tsv (the PNG form is several GB)" >&2
    fi
  else
    echo "no hd_textures/$P: the build has no pack (the textures row is grey)"
  fi
done
# anything else in the folder (the pilot's flat pack, PNG sources) is not read by the game: say so
for F in "$HD"/*; do
  case "$(basename "$F")" in x4) ;; *) echo "note: $F is not read by the game (only x4/ is); it is shipped as it is" ;; esac
done
