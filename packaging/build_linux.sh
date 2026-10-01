#!/bin/sh
# Linux x86_64 build in Docker (works from an arm64 Mac) -> target/dist/PorscheUnleashed-linux-x86_64-<commit>.tar.gz
# Contents: PorscheUnleashed, run.sh, README.txt, game/ (the game data; see stage_data.sh).
# Cargo registry/target live in the docker volumes nfs5-cargo-reg / nfs5-linux-target (fast rebuilds).
set -e
cd "$(dirname "$0")/.."
ROOT=$(pwd)
COMMIT=$(git rev-parse --short=8 HEAD)
DIST=$ROOT/target/dist
STAGE=$DIST/stage-linux/PorscheUnleashed
mkdir -p "$DIST"
[ -d "$DIST/stage/game" ] || sh packaging/stage_data.sh "$DIST/stage/game"
docker build -q --platform linux/amd64 -t nfs5-build-linux -f packaging/Dockerfile.linux packaging >/dev/null
docker run --rm --platform linux/amd64 \
  -v "$ROOT":/src:ro -v nfs5-cargo-reg:/usr/local/cargo/registry -v nfs5-linux-target:/target \
  -v "$DIST":/out -e CARGO_TARGET_DIR=/target -w /src nfs5-build-linux \
  sh -c 'cargo build --release --locked -p porsche && cp /target/release/porsche /out/porsche-linux.bin && strip /out/porsche-linux.bin'
rm -rf "$DIST/stage-linux"
mkdir -p "$STAGE"
mv "$DIST/porsche-linux.bin" "$STAGE/PorscheUnleashed"
chmod +x "$STAGE/PorscheUnleashed"
cp -R "$DIST/stage/game" "$STAGE/game"
cp packaging/run.sh "$STAGE/run.sh"
chmod +x "$STAGE/run.sh"
cp packaging/README-linux.txt "$STAGE/README.txt"
OUT=$DIST/PorscheUnleashed-linux-x86_64-$COMMIT.tar.gz
COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -C "$DIST/stage-linux" -czf "$OUT" PorscheUnleashed
rm -rf "$DIST/stage-linux"
ls -lh "$OUT"
