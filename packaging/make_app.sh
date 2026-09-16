#!/bin/sh
# Build PorscheUnleashed.app (Apple Silicon) from the release binary. Original game data stays external
# (GAME_PATH.md); set NFS5_GAME_PATH or keep the default location.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p porsche
APP=target/PorscheUnleashed.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/porsche "$APP/Contents/MacOS/PorscheUnleashed"
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
