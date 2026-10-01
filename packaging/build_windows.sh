#!/bin/sh
# Windows x64 cross-build (cargo-xwin, msvc target, in Docker) -> target/dist/PorscheUnleashed-windows-x64-<commit>.zip
# Contents: PorscheUnleashed.exe, README.txt, game/ (the game data; see stage_data.sh).
# The MSVC CRT / SDK files xwin downloads are cached in the volume nfs5-xwin.
set -e
cd "$(dirname "$0")/.."
ROOT=$(pwd)
COMMIT=$(git rev-parse --short=8 HEAD)
DIST=$ROOT/target/dist
STAGE=$DIST/stage-win/PorscheUnleashed
mkdir -p "$DIST"
[ -d "$DIST/stage/game" ] || sh packaging/stage_data.sh "$DIST/stage/game"
docker build -q --platform linux/amd64 -t nfs5-build-windows -f packaging/Dockerfile.windows packaging >/dev/null
docker run --rm --platform linux/amd64 \
  -v "$ROOT":/src:ro -v nfs5-cargo-reg:/usr/local/cargo/registry -v nfs5-win-target:/target -v nfs5-xwin:/xwin \
  -v "$DIST":/out -e CARGO_TARGET_DIR=/target -e XWIN_CACHE_DIR=/xwin -w /src nfs5-build-windows \
  sh -c 'cargo xwin build --release --locked --xwin-arch x86_64 --target x86_64-pc-windows-msvc -p porsche && cp /target/x86_64-pc-windows-msvc/release/porsche.exe /out/porsche-win.exe'
rm -rf "$DIST/stage-win"
mkdir -p "$STAGE"
mv "$DIST/porsche-win.exe" "$STAGE/PorscheUnleashed.exe"
cp -R "$DIST/stage/game" "$STAGE/game"
cp packaging/README-windows.txt "$STAGE/README.txt"
OUT=$DIST/PorscheUnleashed-windows-x64-$COMMIT.zip
rm -f "$OUT"
(cd "$DIST/stage-win" && zip -qrX "$OUT" PorscheUnleashed)
rm -rf "$DIST/stage-win"
ls -lh "$OUT"
