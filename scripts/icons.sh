#!/usr/bin/env bash
# Render the app icon from icons/*.svg (needs rsvg-convert: brew install librsvg).
#   - Android: adaptive icon (blue background + white "D" foreground, also used
#     as the monochrome layer for themed icons) and a legacy PNG for older
#     launchers, under android/res.
#   - Desktop: icons/diari-256.png, the Slint window icon.
set -euo pipefail
cd "$(dirname "$0")/.."
RES=android/res
BG="#4A63A0"

# density: legacy icon px (48dp), adaptive layer px (108dp)
for d in "mdpi 48 108" "hdpi 72 162" "xhdpi 96 216" "xxhdpi 144 324" "xxxhdpi 192 432"; do
  set -- $d
  mkdir -p "$RES/mipmap-$1"
  rsvg-convert -w "$2" -h "$2" icons/d-icon.svg -o "$RES/mipmap-$1/ic_launcher.png"
  rsvg-convert -w "$3" -h "$3" icons/d-foreground.svg -o "$RES/mipmap-$1/ic_launcher_foreground.png"
done

mkdir -p "$RES/mipmap-anydpi-v26" "$RES/values"
cat > "$RES/mipmap-anydpi-v26/ic_launcher.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
    <monochrome android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
XML
cat > "$RES/values/ic_launcher_background.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">$BG</color>
</resources>
XML

rsvg-convert -w 256 -h 256 icons/d-icon.svg -o icons/diari-256.png

# Button icons (white on transparent; the UI colorizes them for the theme).
for i in gear pencil clip menu mic camera sync; do
  rsvg-convert -w 64 -h 64 "icons/$i.svg" -o "icons/$i-64.png"
done
echo "icons written to $RES and icons/diari-256.png"
