#!/bin/sh
# Build PorscheUnleashed.app (Apple Silicon) from the release binary.
#
#   make_app.sh              the code only; the original game data stays external (GAME_PATH.md):
#                            set NFS5_GAME_PATH or keep the default location
#   make_app.sh --with-data  a self-contained PorscheUnleashed-Full.app that also carries the data
#                            the port reads (GameData, FEData, fe.txt) in
#                            Contents/Resources/game, plus madtables.bin — the movie decoder's tables
#                            extracted from nfs5.exe (the executable itself is NOT bundled). The assets are EA's: for your own machines only,
#                            never publish or share this bundle.
#
# The game data is taken from $NFS5_GAME_PATH or ~/Games/NFS-Porsche-Unleashed/game_info/data and
# only read (copied), never modified.
set -e
cd "$(dirname "$0")/.."
WITH_DATA=0
[ "$1" = "--with-data" ] && WITH_DATA=1
cargo build --release -p porsche
if [ "$WITH_DATA" = 1 ]; then APP=target/PorscheUnleashed-Full.app; else APP=target/PorscheUnleashed.app; fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/porsche "$APP/Contents/MacOS/PorscheUnleashed"
if [ "$WITH_DATA" = 1 ]; then
  SRC="${NFS5_GAME_PATH:-$HOME/Games/NFS-Porsche-Unleashed/game_info/data}"
  [ -d "$SRC/GameData" ] && [ -d "$SRC/FEData" ] || { echo "game data not found at $SRC (set NFS5_GAME_PATH)" >&2; exit 1; }
  DST="$APP/Contents/Resources/game"
  mkdir -p "$DST"
  cp -R "$SRC/GameData" "$SRC/FEData" "$DST/"
  # files the port has no reader for at all: SourceSafe markers, Excel sheets, C headers and three
  # formats no crate parses (.dtx, .eng, .pfn)
  find "$DST" -type f \( -iname 'vssver.scc' -o -iname '*.xls' -o -iname '*.h' -o -iname '*.dtx' -o -iname '*.eng' -o -iname '*.pfn' \) -delete
  cargo run -q --release -p nfs-formats --example extract_mad_tables -- "$SRC/nfs5.exe" "$DST/madtables.bin"
  [ -f "$SRC/fe.txt" ] && cp "$SRC/fe.txt" "$DST/"
  # no regs/*.reg: the port reads only "Language" from them, and a missing value is "English",
  # which is what this install's own reg.reg holds (GamePaths::language, 0x4B73F0)
  chmod -R u+w "$DST"
fi
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Porsche Unleashed</string>
  <key>CFBundleDisplayName</key><string>Porsche Unleashed</string>
  <key>CFBundleIdentifier</key><string>dev.porsche-re.PorscheUnleashed</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleExecutable</key><string>PorscheUnleashed</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
</dict></plist>
PLIST
echo "built $APP"
file "$APP/Contents/MacOS/PorscheUnleashed"
