#!/bin/bash
# Собирает .app-бандл для macOS: бинарник, Info.plist и иконка из icons/app.ico.
# Источник иконки — кадр 256×256, крупные размеры растягиваются из него.
# Использование: tools/make_app.sh [--debug] (по умолчанию release).
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE=release
if [ "${1:-}" = "--debug" ]; then
  PROFILE=debug
fi

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
NAME="Яндекс Музыка RS"
APP="target/$PROFILE/yamusic-rs.app"

cargo build "--$PROFILE"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/$PROFILE/yamusic-rs" "$APP/Contents/MacOS/yamusic-rs"

# Иконка: app.ico → PNG 256 → все размеры → icns.
ICONSET="$APP/Contents/Resources/AppIcon.iconset"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP" "$ICONSET"' EXIT
mkdir -p "$ICONSET"
sips -s format png icons/app.ico --out "$TMP/base.png" >/dev/null
for n in 16 32 64 128 256 512 1024; do
  sips -z "$n" "$n" "$TMP/base.png" --out "$TMP/$n.png" >/dev/null
done
cp "$TMP/16.png"   "$ICONSET/icon_16x16.png"
cp "$TMP/32.png"   "$ICONSET/icon_16x16@2x.png"
cp "$TMP/32.png"   "$ICONSET/icon_32x32.png"
cp "$TMP/64.png"   "$ICONSET/icon_32x32@2x.png"
cp "$TMP/128.png"  "$ICONSET/icon_128x128.png"
cp "$TMP/256.png"  "$ICONSET/icon_128x128@2x.png"
cp "$TMP/256.png"  "$ICONSET/icon_256x256.png"
cp "$TMP/512.png"  "$ICONSET/icon_256x256@2x.png"
cp "$TMP/512.png"  "$ICONSET/icon_512x512.png"
cp "$TMP/1024.png" "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>$NAME</string>
	<key>CFBundleDisplayName</key><string>$NAME</string>
	<key>CFBundleExecutable</key><string>yamusic-rs</string>
	<key>CFBundleIdentifier</key><string>ru.spolzermod.yamusic-rs</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>LSMinimumSystemVersion</key><string>12.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>LSApplicationCategoryType</key><string>public.app-category.music</string>
</dict>
</plist>
EOF

# Ad-hoc подпись: без неё передача бандла на другой мак упрётся в Gatekeeper.
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "codesign не сработал — локально запустится и так"

echo "Готово: $APP"
echo "Запуск: open \"$APP\""
