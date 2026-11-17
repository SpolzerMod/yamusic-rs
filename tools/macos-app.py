#!/usr/bin/env python3
"""Упаковать готовую release-сборку в macOS .app (Python 3.11+)."""
import pathlib
import plistlib
import shutil
import subprocess
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
version = tomllib.loads((root / 'Cargo.toml').read_text())['package']['version']
app = root / 'target' / 'macos' / 'Яндекс Музыка RS.app'
contents = app / 'Contents'
(contents / 'MacOS').mkdir(parents=True, exist_ok=True)
(contents / 'Resources').mkdir(exist_ok=True)
shutil.copy2(root / 'target/release/yamusic-rs', contents / 'MacOS/yamusic-rs')
iconset = root / 'target' / 'macos' / 'AppIcon.iconset'
iconset.mkdir(exist_ok=True)
for size in (16, 32, 128, 256, 512):
    for scale in (1, 2):
        pixels = size * scale
        suffix = '@2x' if scale == 2 else ''
        subprocess.run(['sips', '-z', str(pixels), str(pixels),
            str(root / 'packaging/icons/256.png'), '--out',
            str(iconset / f'icon_{size}x{size}{suffix}.png')], check=True, stdout=subprocess.DEVNULL)
subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o',
    str(contents / 'Resources/AppIcon.icns')], check=True)
with (contents / 'Info.plist').open('wb') as f:
    plistlib.dump(dict(CFBundleName='Яндекс Музыка RS', CFBundleDisplayName='Яндекс Музыка RS',
        CFBundleIdentifier='io.github.spolzermod.yamusic-rs', CFBundleExecutable='yamusic-rs',
        CFBundlePackageType='APPL', CFBundleIconFile='AppIcon', CFBundleShortVersionString=version, CFBundleVersion=version,
        NSHighResolutionCapable=True, NSPrincipalClass='NSApplication'), f)
subprocess.run(['codesign', '--force', '--deep', '--sign', '-', str(app)], check=True)
print(app)
