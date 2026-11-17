#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p target/macos-tests
xcrun clang -fobjc-arc -fblocks -mmacosx-version-min=13.0 \
    -Wall -Wextra -Werror -Wno-unused-parameter \
    tests/macos_bridge.m src/sys/macos/native/app.m src/sys/macos/native/media.m \
    src/sys/macos/native/autostart.m -framework Cocoa -framework MediaPlayer \
    -framework ServiceManagement -o target/macos-tests/bridge
./target/macos-tests/bridge
