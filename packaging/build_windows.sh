#!/bin/sh
# Windows x64 cross-build (cargo-xwin, msvc target, in Docker) -> target/dist/<APP_ID>-windows-x64-<version>[-with-data].zip
# Contents: <APP_ID>.exe, dxcompiler.dll, dxil.dll, licenses/dxc/, README.txt, LICENSE.txt, THIRD-PARTY-LICENSES.txt,
# game/ (empty: where the player's data goes; with --with-data the game data itself, see stage_data.sh: private use only).
# Also next to the exe: dxcompiler.dll + dxil.dll (the official DirectX Shader Compiler, pinned below, checksum verified, cached in
# target/dxc-cache) and its licences under licenses/dxc/. The game loads them for the DirectX 12 backend (wgpu Dx12Compiler::DynamicDxc,
# crates/nfs-renderer/src/gpu.rs `win_dxc_path`) and falls back to FXC when they are missing. (`static-dxc` was not chosen: it builds
# mach-dxcompiler-rs, a multi-hundred-MB C++ build, from source with clang under cargo-xwin, and adds ~30 MB to the exe.)
# The MSVC CRT / SDK files xwin downloads are cached in the volume nfs5-xwin.
# PACK_NATIVE=1 builds without the container (CI).
set -e
cd "$(dirname "$0")/.."
. packaging/common.sh
parse_pack_args "$@"
ROOT=$(pwd)
# <version>: PACK_VERSION (a release sets it to the tag, e.g. v0.1.0), else the commit
COMMIT=${PACK_VERSION:-$(git rev-parse --short=8 HEAD)}
DIST=$ROOT/target/dist
STAGE=$DIST/stage-win/$APP_ID
DXC_TAG=v1.9.2602.24
DXC_ZIP=dxc_2026_05_27.zip
DXC_URL=https://github.com/microsoft/DirectXShaderCompiler/releases/download/$DXC_TAG/$DXC_ZIP
DXC_SHA256=cf658aacf070d3045e31b8f1f8a696c2945f37c1095019481ef7c513368db3b4
DXC_CACHE=$ROOT/target/dxc-cache
mkdir -p "$DIST" "$DXC_CACHE"
sha256_of() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
if [ ! -f "$DXC_CACHE/$DXC_ZIP" ] || [ "$(sha256_of "$DXC_CACHE/$DXC_ZIP")" != "$DXC_SHA256" ]; then
  curl -fsSL -o "$DXC_CACHE/$DXC_ZIP" "$DXC_URL"
  [ "$(sha256_of "$DXC_CACHE/$DXC_ZIP")" = "$DXC_SHA256" ] || { echo "DXC checksum mismatch: $DXC_ZIP" >&2; rm -f "$DXC_CACHE/$DXC_ZIP"; exit 1; }
fi
rm -rf "$DXC_CACHE/unpacked"
mkdir -p "$DXC_CACHE/unpacked"
unzip -q "$DXC_CACHE/$DXC_ZIP" 'bin/x64/dxcompiler.dll' 'bin/x64/dxil.dll' 'LICENSE-*.txt' 'inc/hlsl/LICENSE.txt' -d "$DXC_CACHE/unpacked"
if [ "$WITH_DATA" = 1 ]; then [ -d "$DIST/stage/game" ] || sh packaging/stage_data.sh "$DIST/stage/game"; fi
if [ "${PACK_NATIVE:-0}" = 1 ]; then
  # PACK_NATIVE=1: build right here (a Linux CI runner with clang / lld / llvm, the msvc target and cargo-xwin, see Dockerfile.windows)
  cargo xwin build --release --locked --xwin-arch x86_64 --target x86_64-pc-windows-msvc -p open-unleashed
  cp "${CARGO_TARGET_DIR:-target}/x86_64-pc-windows-msvc/release/open-unleashed.exe" "$DIST/app-win.exe"
else
  docker build -q --platform linux/amd64 -t nfs5-build-windows -f packaging/Dockerfile.windows packaging >/dev/null
  docker run --rm --platform linux/amd64 \
    -v "$ROOT":/src:ro -v nfs5-cargo-reg:/usr/local/cargo/registry -v nfs5-win-target:/target -v nfs5-xwin:/xwin \
    -v "$DIST":/out -e CARGO_TARGET_DIR=/target -e XWIN_CACHE_DIR=/xwin -w /src nfs5-build-windows \
    sh -c 'cargo xwin build --release --locked --xwin-arch x86_64 --target x86_64-pc-windows-msvc -p open-unleashed && cp /target/x86_64-pc-windows-msvc/release/open-unleashed.exe /out/app-win.exe'
fi
rm -rf "$DIST/stage-win"
mkdir -p "$STAGE"
mv "$DIST/app-win.exe" "$STAGE/$APP_ID.exe"
if [ "$WITH_DATA" = 1 ]; then cp -R "$DIST/stage/game" "$STAGE/game"; else stage_game_dir "$STAGE/game"; fi
cp "$DXC_CACHE/unpacked/bin/x64/dxcompiler.dll" "$DXC_CACHE/unpacked/bin/x64/dxil.dll" "$STAGE/"
mkdir -p "$STAGE/licenses/dxc"
cp "$DXC_CACHE"/unpacked/LICENSE-*.txt "$STAGE/licenses/dxc/"
printf 'DirectX Shader Compiler %s (%s)\nhttps://github.com/microsoft/DirectXShaderCompiler\nThe dxcompiler.dll and dxil.dll next to the exe are the unmodified official x64 binaries. Licence: LLVM Apache-2.0 with LLVM exceptions, MIT, Microsoft (see the LICENSE-*.txt here).\n' "$DXC_TAG" "$DXC_ZIP" > "$STAGE/licenses/dxc/README.txt"
brand_readme packaging/README-windows.txt > "$STAGE/README.txt"
stage_legal "$STAGE" x86_64-pc-windows-msvc "$DXC_CACHE/unpacked"
OUT=$DIST/$APP_ID-windows-x64-$COMMIT$SUFFIX.zip
rm -f "$OUT"
(cd "$DIST/stage-win" && zip -qrX "$OUT" "$APP_ID")
rm -rf "$DIST/stage-win"
ls -lh "$OUT"
