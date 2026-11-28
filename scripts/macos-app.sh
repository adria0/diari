#!/usr/bin/env bash
# Build diari.app, the macOS app bundle, and optionally install it.
#   scripts/macos-app.sh            # -> target/release/diari.app
#   scripts/macos-app.sh install    # ... and copy it to /Applications
# The icon comes from icons/d-icon.svg (needs rsvg-convert: brew install
# librsvg); without it, from icons/diari-256.png.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release --locked

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP=target/release/diari.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/diari "$APP/Contents/MacOS/diari"

# Icon: every size macOS asks for, in an .icns.
ICONSET="$(mktemp -d)/diari.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  for scale in 1 2; do
    px=$((s * scale))
    name="icon_${s}x${s}.png"
    [[ "$scale" == 2 ]] && name="icon_${s}x${s}@2x.png"
    if command -v rsvg-convert >/dev/null; then
      rsvg-convert -w "$px" -h "$px" icons/d-icon.svg -o "$ICONSET/$name"
    else
      sips -z "$px" "$px" icons/diari-256.png --out "$ICONSET/$name" >/dev/null
    fi
  done
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/diari.icns"
rm -rf "$(dirname "$ICONSET")"

# The usage strings are shown when macOS asks for the microphone (voice
# notes) and the Documents folder (where the notes are).
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>diari</string>
  <key>CFBundleDisplayName</key><string>diari</string>
  <key>CFBundleIdentifier</key><string>dev.perb.diari</string>
  <key>CFBundleExecutable</key><string>diari</string>
  <key>CFBundleIconFile</key><string>diari</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>diari records voice notes and transcribes them on this Mac.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>diari keeps your notes in Documents/diari.</string>
</dict>
</plist>
PLIST

# Ad-hoc signature, so macOS remembers the permissions it grants.
codesign --force --sign - "$APP"
echo "built $APP"

if [[ "${1:-}" == install ]]; then
  if pgrep -xq diari; then
    echo "diari is running: quit it first" >&2
    exit 1
  fi
  rm -rf /Applications/diari.app
  ditto "$APP" /Applications/diari.app
  echo "installed /Applications/diari.app"
fi
