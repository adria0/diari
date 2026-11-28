#!/usr/bin/env bash
# Take the README screenshots, from demo notes so no real notes show.
#   scripts/screenshots.sh macos     # -> docs/screenshots/macos.png
#   scripts/screenshots.sh android   # -> docs/screenshots/android.png (needs a device or emulator on adb)
#   scripts/screenshots.sh           # both (android skipped when no device)
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=docs/screenshots
mkdir -p "$OUT"
WHAT="${1:-all}"

# Demo notes repo: a journal with today's note, and a second workspace.
demo_notes() {
  local dir="$1" today
  today="$(date +%Y_%m_%d)"
  mkdir -p "$dir/diari" "$dir/feina"
  printf '# workspace\n```yaml\nname: Diari\n```\n' > "$dir/diari/CONFIG.md"
  printf '# workspace\n```yaml\nname: Feina\ncolor: "#3b82f6"\n```\n' > "$dir/feina/CONFIG.md"
  cat > "$dir/diari/$today.md" <<MD
# $today

## Calendar
- 09:30-10:00 Standup
- 13:00-14:00 Lunch with **Marta**

## To do
- [x] Book train tickets
- [ ] Review the [[Roadmap]]
- [ ] Buy *good* coffee beans

- 08:12 Morning run, 5 km along the river.
- 11:40 Idea: **weekly review** note every Friday.
MD
  cat > "$dir/feina/Roadmap.md" <<'MD'
# Roadmap

- [x] Git sync over SSH
- [x] Voice notes
- [ ] Desktop drag and drop for folders
MD
  printf '# Recipes\n\n- Pa amb tomàquet\n- Escalivada\n' > "$dir/diari/Recipes.md"
}

macos() {
  cargo build --release
  local home; home="$(mktemp -d)"
  demo_notes "$home/Documents/diari"
  # dirs:: resolves Application Support and Documents from $HOME, so the app
  # starts with a fresh config and the demo notes only.
  HOME="$home" ./target/release/diari >/dev/null 2>&1 &
  local pid=$!
  trap 'kill $pid 2>/dev/null || true; rm -rf "$home"' RETURN
  local wid=""
  for _ in $(seq 40); do
    perl -e 'select(undef,undef,undef,0.25)'
    wid="$(swift - "$pid" <<'SWIFT' 2>/dev/null || true
import CoreGraphics
import Foundation
let pid = Int(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as! [[String: Any]]
if let w = list.first(where: { ($0[kCGWindowOwnerPID as String] as? Int) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }) {
  print(w[kCGWindowNumber as String] as! Int)
}
SWIFT
)"
    [[ -n "$wid" ]] && break
  done
  [[ -n "$wid" ]] || { echo "diari window not found" >&2; return 1; }
  perl -e 'select(undef,undef,undef,1.5)' # let it render
  screencapture -x -o -l "$wid" "$OUT/macos.png"
  sips -Z 1200 "$OUT/macos.png" >/dev/null # retina size is too big for a README
  echo "wrote $OUT/macos.png"
}

android() {
  local adb="${ANDROID_HOME:-$HOME/Library/Android/sdk}/platform-tools/adb"
  command -v adb >/dev/null && adb=adb
  if ! "$adb" get-state >/dev/null 2>&1; then
    echo "no Android device on adb, skipping android" >&2
    [[ "$WHAT" == android ]] && return 1 || return 0
  fi
  # The app's notes are app-private, so this shows whatever is on the device:
  # use an emulator with demo notes, not your phone.
  "$adb" shell monkey -p dev.perb.app -c android.intent.category.LAUNCHER 1 >/dev/null
  perl -e 'select(undef,undef,undef,3)'
  "$adb" exec-out screencap -p > "$OUT/android.png"
  sips -Z 1000 "$OUT/android.png" >/dev/null
  echo "wrote $OUT/android.png"
}

case "$WHAT" in
  macos) macos ;;
  android) android ;;
  all) macos; android ;;
  *) echo "usage: $0 [macos|android|all]" >&2; exit 1 ;;
esac
