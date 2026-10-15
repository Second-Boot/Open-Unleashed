#!/bin/sh
# macOS universal (arm64 + x86_64) build -> target/dist/<APP_ID>-macos-universal-<version>[-with-data].zip
# Contents: <APP_ID>.app, game/ (where the player's data goes), README.txt, LICENSE.txt, THIRD-PARTY-LICENSES.txt.
#   --with-data   <APP_ID>-WithData.app with the game data inside instead (make_app.sh --with-data): private use only
set -e
cd "$(dirname "$0")/.."
. packaging/common.sh
parse_pack_args "$@"
ROOT=$(pwd)
# <version>: PACK_VERSION (a release sets it to the tag, e.g. v0.1.0), else the commit
COMMIT=${PACK_VERSION:-$(git rev-parse --short=8 HEAD)}
DIST=$ROOT/target/dist
STAGE=$DIST/stage-mac/$APP_ID
mkdir -p "$DIST"
rm -rf "$DIST/stage-mac"
mkdir -p "$STAGE"
if [ "$WITH_DATA" = 1 ]; then
  sh packaging/make_app.sh --with-data --universal
  ditto --norsrc --noextattr "target/$APP_ID-WithData.app" "$STAGE/$APP_ID-WithData.app"
else
  sh packaging/make_app.sh --universal
  ditto --norsrc --noextattr "target/$APP_ID.app" "$STAGE/$APP_ID.app"
  stage_game_dir "$STAGE/game"
fi
brand_readme packaging/README-macos.txt > "$STAGE/README.txt"
stage_legal "$STAGE" aarch64-apple-darwin
OUT=$DIST/$APP_ID-macos-universal-$COMMIT$SUFFIX.zip
rm -f "$OUT"
(cd "$DIST/stage-mac" && ditto -c -k --norsrc --noextattr --keepParent "$APP_ID" "$OUT")
rm -rf "$DIST/stage-mac"
ls -lh "$OUT"
