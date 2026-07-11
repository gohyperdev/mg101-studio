#!/bin/zsh
set -euo pipefail

ROOT=${0:A:h:h}
cd "$ROOT"

swift build -c release --arch arm64

BUILD="$ROOT/.build/arm64-apple-macosx/release"
APP="$ROOT/dist/MG101Studio.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$ROOT/Resources/Info.plist" "$APP/Contents/Info.plist"
cp "$BUILD/MG101Studio" "$APP/Contents/MacOS/MG101Studio"
cp "$BUILD/MG101MCP" "$APP/Contents/MacOS/MG101MCP"
cp -R "$BUILD/MG101Studio_MG101Core.bundle" \
    "$APP/Contents/Resources/MG101Studio_MG101Core.bundle"

codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

echo "$APP"
