#!/bin/sh
# Stage the game data the port reads into $1 (default target/dist/stage/game): the same set
# make_app.sh --with-data puts in the .app (GameData, FEData, fe.txt, madtables.bin). The source
# ($NFS5_GAME_PATH or ~/Games/NFS-Porsche-Unleashed/game_info/data) is only read.
set -e
cd "$(dirname "$0")/.."
DST="${1:-target/dist/stage/game}"
SRC="${NFS5_GAME_PATH:-$HOME/Games/NFS-Porsche-Unleashed/game_info/data}"
[ -d "$SRC/GameData" ] && [ -d "$SRC/FEData" ] || { echo "game data not found at $SRC (set NFS5_GAME_PATH)" >&2; exit 1; }
rm -rf "$DST"
mkdir -p "$DST"
cp -R "$SRC/GameData" "$SRC/FEData" "$DST/"
find "$DST" -type f \( -iname 'vssver.scc' -o -iname '*.xls' -o -iname '*.h' -o -iname '*.dtx' -o -iname '*.eng' -o -iname '*.pfn' \) -delete
cargo run -q --release -p nfs-formats --example extract_mad_tables -- "$SRC/nfs5.exe" "$DST/madtables.bin"
[ -f "$SRC/fe.txt" ] && cp "$SRC/fe.txt" "$DST/"
# the game's version ("3.5"): the network session name carries it, and a host refuses any other
[ -f "$SRC/version.txt" ] && cp "$SRC/version.txt" "$DST/"
chmod -R u+w "$DST"
du -sh "$DST"
