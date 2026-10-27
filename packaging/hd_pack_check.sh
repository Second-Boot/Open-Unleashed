#!/bin/sh
# ENHANCEMENT E41: report the HD texture packs that ride along in a staged data dir (sourced by stage_data.sh and make_app.sh;
# usage: hd_pack_check.sh <dst>). The packs are `enhanced/hd_textures/x2` and `enhanced/hd_textures/x4`, each the output of
# `hd-pack bc7` (KTX2 / BC7 files, plus a handful of PNGs where a side is not a multiple of 4), copied by the `cp -R remaster` of the
# caller; neither is in git (`/enhanced/hd_textures/` is ignored). A pack made of PNGs would ship several GB: warn.
DST=$1
HD="$DST/enhanced/hd_textures"
if [ ! -d "$HD" ]; then
  echo "no enhanced/hd_textures: the build ships the original textures only"
  exit 0
fi
for P in x2 x4; do
  if [ -d "$HD/$P" ]; then
    K=$(find "$HD/$P" -name '*.ktx2' | wc -l | tr -d ' ')
    G=$(find "$HD/$P" -name '*.png' | wc -l | tr -d ' ')
    echo "HD texture pack $P included: $(du -sh "$HD/$P" | cut -f1) ($K ktx2 / BC7, $G png)"
    if [ "$G" -gt 0 ] && [ "$G" -gt $((K / 10)) ]; then
      echo "WARNING: $HD/$P holds mostly PNGs - convert it with: hd-pack bc7 <png pack> <out> --manifest manifest.tsv (the PNG form is several GB)" >&2
    fi
  else
    echo "no enhanced/hd_textures/$P: the build has no $P level (the game falls back to the original textures there)"
  fi
done
# anything else in the folder (the pilot's flat pack, PNG sources) is not read by the game: say so
for F in "$HD"/*; do
  case "$(basename "$F")" in x2|x4) ;; *) echo "note: $F is not read by the game (only x2/ and x4/ are); it is shipped as it is" ;; esac
done
