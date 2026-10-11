#!/bin/sh
# Windows x64 cross-build (cargo-xwin, msvc target, in Docker) -> target/dist/<APP_ID>-windows-x64-<commit>[-with-data].zip
# Contents: <APP_ID>.exe, README.txt, LICENSE.txt, THIRD-PARTY-LICENSES.txt,
# game/ (empty: where the player's data goes; with --with-data the game data itself, see stage_data.sh: private use only).
# The MSVC CRT / SDK files xwin downloads are cached in the volume nfs5-xwin.
set -e
cd "$(dirname "$0")/.."
. packaging/common.sh
parse_pack_args "$@"
ROOT=$(pwd)
COMMIT=$(git rev-parse --short=8 HEAD)
DIST=$ROOT/target/dist
STAGE=$DIST/stage-win/$APP_ID
mkdir -p "$DIST"
if [ "$WITH_DATA" = 1 ]; then [ -d "$DIST/stage/game" ] || sh packaging/stage_data.sh "$DIST/stage/game"; fi
docker build -q --platform linux/amd64 -t nfs5-build-windows -f packaging/Dockerfile.windows packaging >/dev/null
docker run --rm --platform linux/amd64 \
  -v "$ROOT":/src:ro -v nfs5-cargo-reg:/usr/local/cargo/registry -v nfs5-win-target:/target -v nfs5-xwin:/xwin \
  -v "$DIST":/out -e CARGO_TARGET_DIR=/target -e XWIN_CACHE_DIR=/xwin -w /src nfs5-build-windows \
  sh -c 'cargo xwin build --release --locked --xwin-arch x86_64 --target x86_64-pc-windows-msvc -p porsche && cp /target/x86_64-pc-windows-msvc/release/porsche.exe /out/porsche-win.exe'
rm -rf "$DIST/stage-win"
mkdir -p "$STAGE"
mv "$DIST/porsche-win.exe" "$STAGE/$APP_ID.exe"
if [ "$WITH_DATA" = 1 ]; then cp -R "$DIST/stage/game" "$STAGE/game"; else stage_game_dir "$STAGE/game"; fi
brand_readme packaging/README-windows.txt > "$STAGE/README.txt"
stage_legal "$STAGE" x86_64-pc-windows-msvc
OUT=$DIST/$APP_ID-windows-x64-$COMMIT$SUFFIX.zip
rm -f "$OUT"
(cd "$DIST/stage-win" && zip -qrX "$OUT" "$APP_ID")
rm -rf "$DIST/stage-win"
ls -lh "$OUT"
