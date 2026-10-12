#!/bin/sh
# Build <APP_ID>.app (Apple Silicon, or arm64+x86_64 with --universal) from the release binary
# (the name: packaging/brand.sh).
#
#   make_app.sh              the code only (the public build); the player's own game data stays external:
#                            game/ beside the .app, NFS5_GAME_PATH or the default location (docs/en/game-data.md)
#   make_app.sh --universal  also build x86_64 and ship one arm64+x86_64 (lipo) binary, so the app
#                            runs on Intel Macs too (needs `rustup target add x86_64-apple-darwin`);
#                            combines with --with-data in any order
#   make_app.sh --with-data  a self-contained <APP_ID>-WithData.app that also carries the data
#                            the port reads (GameData, FEData, fe.txt) in
#                            Contents/Resources/game, plus madtables.bin — the movie decoder's tables
#                            extracted from nfs5.exe (the executable itself is NOT bundled). The assets are EA's: for your own machines only,
#                            never publish or share this bundle.
#
# The game data is taken from $NFS5_GAME_PATH or ~/Games/NFS-Porsche-Unleashed/game_info/data and
# only read (copied), never modified.
set -e
cd "$(dirname "$0")/.."
. packaging/brand.sh
WITH_DATA=0
UNIVERSAL=0
for a in "$@"; do
  case "$a" in
    --with-data) WITH_DATA=1 ;;
    --universal) UNIVERSAL=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done
cargo build --release -p open-unleashed
if [ "$UNIVERSAL" = 1 ]; then
  cargo build --release -p open-unleashed --target x86_64-apple-darwin
fi
if [ "$WITH_DATA" = 1 ]; then APP=target/$APP_ID-WithData.app; else APP=target/$APP_ID.app; fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "$UNIVERSAL" = 1 ]; then
  lipo -create target/release/open-unleashed target/x86_64-apple-darwin/release/porsche \
    -output "$APP/Contents/MacOS/$APP_ID"
else
  cp target/release/open-unleashed "$APP/Contents/MacOS/$APP_ID"
fi
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
  <key>CFBundleName</key><string>$APP_TITLE</string>
  <key>CFBundleDisplayName</key><string>$APP_TITLE</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleExecutable</key><string>$APP_ID</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <!-- Network play (PLATFORM GLUE, not in the original): from macOS 15 (and 14 for apps started from
       the Finder) any UDP to or from a local-network address - the game's broadcast that finds a
       host on the LAN (UDP 9442), the join and the race traffic - is held back until the user has
       allowed "Local Network" for the app; the text below is what the system shows in that prompt.
       Without this key the prompt has no purpose string and the permission ends up denied, which
       shows as sends failing with EHOSTUNREACH ("No route to host") and an empty games list.
       NSBonjourServices is NOT needed: it only lists Bonjour/mDNS service types an app browses, and
       this game does no service discovery, just plain UDP broadcast and unicast.
       (No multicast entitlement either: com.apple.developer.networking.multicast is for multicast
       groups, and the game broadcasts.) -->
  <key>NSLocalNetworkUsageDescription</key><string>$APP_TITLE looks for and joins network races on your local network (UDP port 9442).</string>
</dict></plist>
PLIST
# Seal the bundle with an ad-hoc signature (PLATFORM GLUE): Apple Silicon runs only signed code, and
# the Local Network permission and the firewall's "allow incoming connections" answer are remembered
# per signed app identity, so an unsealed bundle whose Info.plist changed asks again every build.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "codesign failed - the app may not start on Apple Silicon" >&2
echo "built $APP"
file "$APP/Contents/MacOS/$APP_ID"
