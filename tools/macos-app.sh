#!/bin/bash
# Собирает бандл .app и образ .dmg для macOS из уже собранного бинарника.
# Если собраны оба таргета (aarch64-apple-darwin и x86_64-apple-darwin), бинарник
# склеивается в универсальный через lipo, иначе берётся target/release/yamusic-rs.
# Подпись ad-hoc: без сертификата Apple Gatekeeper при первом запуске спросит разрешение.
# Результат: target/macos/Яндекс Музыка RS.app и target/macos/yamusic-rs-macos.dmg.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="Яндекс Музыка RS"
BUNDLE_ID="io.github.spolzermod.yamusic-rs"
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=target/macos
APP="$OUT/$NAME.app"
DMG="$OUT/yamusic-rs-macos.dmg"

ARM=target/aarch64-apple-darwin/release/yamusic-rs
X86=target/x86_64-apple-darwin/release/yamusic-rs

rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

if [ -f "$ARM" ] && [ -f "$X86" ]; then
  lipo -create -output "$APP/Contents/MacOS/yamusic-rs" "$ARM" "$X86"
else
  cp target/release/yamusic-rs "$APP/Contents/MacOS/yamusic-rs"
fi
chmod 755 "$APP/Contents/MacOS/yamusic-rs"

# Значок: iconutil собирает .icns из набора PNG. Самый большой исходник — 256 px.
ICONSET="$OUT/app.iconset"
mkdir -p "$ICONSET"
cp packaging/icons/16.png "$ICONSET/icon_16x16.png"
cp packaging/icons/32.png "$ICONSET/icon_16x16@2x.png"
cp packaging/icons/32.png "$ICONSET/icon_32x32.png"
cp packaging/icons/64.png "$ICONSET/icon_32x32@2x.png"
cp packaging/icons/128.png "$ICONSET/icon_128x128.png"
cp packaging/icons/256.png "$ICONSET/icon_128x128@2x.png"
cp packaging/icons/256.png "$ICONSET/icon_256x256.png"
iconutil -c icns -o "$APP/Contents/Resources/app.icns" "$ICONSET"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>ru</string>
  <key>CFBundleExecutable</key>
  <string>yamusic-rs</string>
  <key>CFBundleIconFile</key>
  <string>app</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$NAME</string>
  <key>CFBundleDisplayName</key>
  <string>$NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.music</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
</dict>
</plist>
EOF

codesign --force --sign - --identifier "$BUNDLE_ID" "$APP"

# Образ с ярлыком на «Программы»: приложение ставится перетаскиванием.
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "$NAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG"
rm -rf "$STAGE"

echo "$APP"
echo "$DMG"
