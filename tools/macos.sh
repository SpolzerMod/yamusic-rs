#!/bin/bash
# Собирает .app и .dmg для macOS из уже собранного релизного бинарника.
# Если в target/ есть оба таргета (aarch64-apple-darwin и x86_64-apple-darwin),
# склеивает их через lipo в универсальный бинарник (yamusic-rs-macos-universal.dmg).
# Иначе берёт target/release/yamusic-rs для текущей архитектуры.
# Подпись: MACOS_SIGN_IDENTITY (по умолчанию ad-hoc «-»).
# Результат: target/macos/Яндекс Музыка RS.app и target/macos/yamusic-rs-macos-<arch>.dmg.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP_NAME="Яндекс Музыка RS"
BUNDLE_ID="io.github.spolzermod.yamusic-rs"
OUT="target/macos"
APP="$OUT/$APP_NAME.app"
STAGE="$OUT/dmg"

ARM_BIN="target/aarch64-apple-darwin/release/yamusic-rs"
X64_BIN="target/x86_64-apple-darwin/release/yamusic-rs"
NATIVE_BIN="target/release/yamusic-rs"

rm -rf "$APP" "$STAGE"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [ -f "$ARM_BIN" ] && [ -f "$X64_BIN" ]; then
  ARCH="universal"
  lipo -create -output "$APP/Contents/MacOS/yamusic-rs" "$ARM_BIN" "$X64_BIN"
elif [ -f "$NATIVE_BIN" ]; then
  ARCH=$(uname -m)
  [ "$ARCH" = "aarch64" ] && ARCH="arm64"
  cp "$NATIVE_BIN" "$APP/Contents/MacOS/yamusic-rs"
elif [ -f "$ARM_BIN" ]; then
  ARCH="arm64"
  cp "$ARM_BIN" "$APP/Contents/MacOS/yamusic-rs"
elif [ -f "$X64_BIN" ]; then
  ARCH="x86_64"
  cp "$X64_BIN" "$APP/Contents/MacOS/yamusic-rs"
else
  echo "Релизный бинарник не найден: сначала выполните cargo build --release" >&2
  exit 1
fi
chmod 755 "$APP/Contents/MacOS/yamusic-rs"

# Значок .icns из icons/app.svg (если есть rsvg-convert) или из packaging/icons/*.png.
ICONSET="$OUT/AppIcon.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"

make_png() {
  local size="$1" dst="$2"
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert -w "$size" -h "$size" icons/app.svg -o "$dst"
  elif [ -f "packaging/icons/${size}.png" ]; then
    cp "packaging/icons/${size}.png" "$dst"
  else
    sips -z "$size" "$size" packaging/icons/256.png --out "$dst" >/dev/null
  fi
}

make_png 16   "$ICONSET/icon_16x16.png"
make_png 32   "$ICONSET/icon_16x16@2x.png"
make_png 32   "$ICONSET/icon_32x32.png"
make_png 64   "$ICONSET/icon_32x32@2x.png"
make_png 128  "$ICONSET/icon_128x128.png"
make_png 256  "$ICONSET/icon_128x128@2x.png"
make_png 256  "$ICONSET/icon_256x256.png"
make_png 512  "$ICONSET/icon_256x256@2x.png"
make_png 512  "$ICONSET/icon_512x512.png"
make_png 1024 "$ICONSET/icon_512x512@2x.png"

iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>ru</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleExecutable</key>
  <string>yamusic-rs</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.music</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
</dict>
</plist>
EOF

SIGN_ID="${MACOS_SIGN_IDENTITY:--}"
codesign --force --deep --sign "$SIGN_ID" "$APP"
codesign --verify --deep --strict "$APP"

DMG="$OUT/yamusic-rs-macos-${ARCH}.dmg"
rm -f "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

ls -lh "$DMG"
