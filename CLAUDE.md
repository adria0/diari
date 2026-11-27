# diari

## README screenshots
The README shows screenshots of the app (`docs/screenshots/macos.png`, `docs/screenshots/android.png`). Keep them current:

- **When:** after any change people would see in the app (`ui/*.slint`, new buttons, menus, screens, themes, layout), retake the screenshots before committing, and commit them with the change.
- **How:** `scripts/screenshots.sh macos` (or `android`, or no argument for both). The macOS shot runs the app with a temporary `$HOME` and demo notes, so it never shows real notes or settings.
- **Android:** the script screenshots whatever is on the adb device, and the app's notes are app-private. Only take it on an emulator with demo notes. Never use the user's phone. If there's no such emulator, skip the Android shot and say so.
- **Check before committing:** open each new PNG and look at it. It must show only demo content: no real notes, names, calendar events, SSH keys or git remotes. If it shows anything else, don't commit it, and tell the user.
- **Demo content:** if a new feature needs something on screen to show it, add that to `demo_notes` in `scripts/screenshots.sh` rather than using real data.
- When `android.png` first exists, add it to the `<p>` with the macOS image at the top of `README.md`.
