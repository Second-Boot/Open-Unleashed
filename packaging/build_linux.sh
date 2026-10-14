#!/bin/sh
# Linux x86_64 build in Docker (works from an arm64 Mac) -> target/dist/<APP_ID>-linux-x86_64-<commit>[-with-data].tar.gz
# Contents: <APP_ID>, run.sh, <APP_ID>.png (icon), README.txt, LICENSE.txt, THIRD-PARTY-LICENSES.txt,
# game/ (empty: where the player's data goes; with --with-data the game data itself, see stage_data.sh: private use only).
# Cargo registry/target live in the docker volumes nfs5-cargo-reg / nfs5-linux-target (fast rebuilds).
# PACK_NATIVE=1 builds without the container (CI on a Linux runner).
set -e
cd "$(dirname "$0")/.."
. packaging/common.sh
parse_pack_args "$@"
ROOT=$(pwd)
COMMIT=$(git rev-parse --short=8 HEAD)
DIST=$ROOT/target/dist
STAGE=$DIST/stage-linux/$APP_ID
mkdir -p "$DIST"
if [ "$WITH_DATA" = 1 ]; then [ -d "$DIST/stage/game" ] || sh packaging/stage_data.sh "$DIST/stage/game"; fi
if [ "${PACK_NATIVE:-0}" = 1 ]; then
  # PACK_NATIVE=1: build right here (a Linux CI runner that already has Rust and the -dev packages, see Dockerfile.linux)
  cargo build --release --locked -p open-unleashed
  cp "${CARGO_TARGET_DIR:-target}/release/open-unleashed" "$DIST/app-linux.bin" && strip "$DIST/app-linux.bin"
else
  docker build -q --platform linux/amd64 -t nfs5-build-linux -f packaging/Dockerfile.linux packaging >/dev/null
  docker run --rm --platform linux/amd64 \
    -v "$ROOT":/src:ro -v nfs5-cargo-reg:/usr/local/cargo/registry -v nfs5-linux-target:/target \
    -v "$DIST":/out -e CARGO_TARGET_DIR=/target -w /src nfs5-build-linux \
    sh -c 'cargo build --release --locked -p open-unleashed && cp /target/release/open-unleashed /out/app-linux.bin && strip /out/app-linux.bin'
fi
rm -rf "$DIST/stage-linux"
mkdir -p "$STAGE"
mv "$DIST/app-linux.bin" "$STAGE/$APP_ID"
chmod +x "$STAGE/$APP_ID"
if [ "$WITH_DATA" = 1 ]; then cp -R "$DIST/stage/game" "$STAGE/game"; else stage_game_dir "$STAGE/game"; fi
sed "s/@APP@/$APP_ID/g" packaging/run.sh > "$STAGE/run.sh"
chmod +x "$STAGE/run.sh"
# the icon, for a launcher entry the player makes (packaging/brand/render_icons.py)
cp packaging/brand/icon-256.png "$STAGE/$APP_ID.png"
brand_readme packaging/README-linux.txt > "$STAGE/README.txt"
stage_legal "$STAGE" x86_64-unknown-linux-gnu
OUT=$DIST/$APP_ID-linux-x86_64-$COMMIT$SUFFIX.tar.gz
# bsdtar (macOS) would add AppleDouble / xattr entries without these flags; GNU tar (a Linux runner) has neither flag
if tar --version 2>/dev/null | grep -q bsdtar; then TAR_FLAGS="--no-xattrs --no-mac-metadata"; else TAR_FLAGS=""; fi
COPYFILE_DISABLE=1 tar $TAR_FLAGS -C "$DIST/stage-linux" -czf "$OUT" "$APP_ID"
rm -rf "$DIST/stage-linux"
ls -lh "$OUT"
