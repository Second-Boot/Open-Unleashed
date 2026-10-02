#!/bin/sh
# macOS universal (arm64 + x86_64) build -> target/dist/PorscheUnleashed-macos-universal-<commit>.zip
# Contents: PorscheUnleashed-Full.app (the game data inside, see make_app.sh --with-data), README.txt.
set -e
cd "$(dirname "$0")/.."
ROOT=$(pwd)
COMMIT=$(git rev-parse --short=8 HEAD)
DIST=$ROOT/target/dist
STAGE=$DIST/stage-mac/PorscheUnleashed
mkdir -p "$DIST"
sh packaging/make_app.sh --with-data --universal
rm -rf "$DIST/stage-mac"
mkdir -p "$STAGE"
ditto --norsrc --noextattr target/PorscheUnleashed-Full.app "$STAGE/PorscheUnleashed-Full.app"
cp packaging/README-macos.txt "$STAGE/README.txt"
OUT=$DIST/PorscheUnleashed-macos-universal-$COMMIT.zip
rm -f "$OUT"
(cd "$DIST/stage-mac" && ditto -c -k --norsrc --noextattr --keepParent PorscheUnleashed "$OUT")
rm -rf "$DIST/stage-mac"
ls -lh "$OUT"
