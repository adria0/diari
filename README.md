# diari

Native markdown notes with automatic git sync, for macOS and Android. It's written in Rust, and the UI is Slint (no webview).

## Features
- Lists every `*.md` file in the notes folder, including subfolders. You can search, create, rename and delete notes.
- Plain-text editor that autosaves 1 s after you stop typing, plus a native markdown preview.
- Git sync over SSH, using an ed25519 key the app generates. Sync runs:
  - on startup, on Android resume, and when you tap the status chip
  - 1 min after the last save (each new save restarts the minute)
  - every 5 min
- **Conflicts never lose data.** If two devices edit the same note, the remote version keeps the name and the local version is saved as `name.conflict-<device>-<timestamp>.md`.
- **Files and images (Android).** Share files or images to diari from any app, one or several at once. They're saved in today's note's attachment folder (`2026_11_26.md.files/`), and each one adds a line like `- 09:01 [photo.jpg](2026_11_26.md.files/photo.jpg)`.
  - In the preview, linked images are shown, and other links open when you tap them: web links in the browser, files in the app for their type. On Android, a file is first copied to `Download/diari/` so other apps can read it (Android 10 or later).
  - Attachment folders aren't shown in the note list, are synced with git, move when their note is renamed, and are deleted with it.

## Where things live
| | macOS | Android |
|---|---|---|
| Config, SSH key, known hosts | `~/Library/Application Support/diari/` | app-private storage |
| Notes | `~/Documents/diari/` (configurable) | app-private `notes/` |

diari was first called perb. On macOS, the first launch moves `…/Application Support/perb` and `~/Documents/perb` to the `diari` names if those don't exist yet. On Android the package ID stays `dev.perb.app`, so updates keep the app's data.

## Setup
1. Run the app, open **Settings**, and copy the public key.
2. Add the key to your git host as a **deploy key with write access**. On GitHub that's repo → Settings → Deploy keys.
3. Enter the remote (`git@github.com:you/notes.git`) and the branch, then press **Save & sync**.

Host keys are trusted on first use and stored in `known_hosts.toml`. If a host's key later changes, sync stops with an error.

## Build
```sh
# macOS
cargo run --release

# tests (the git sync tests use local bare repos)
cargo test

# Android (one-time setup)
rustup target add aarch64-linux-android x86_64-linux-android
sdkmanager "ndk;30.0.16248370" "build-tools;35.0.0"
cargo install cargo-apk

scripts/android.sh build                                   # -> target/release/apk/diari.apk
scripts/android.sh run                                     # install + launch on device
TARGET=x86_64-linux-android scripts/android.sh run         # x86_64 emulator
adb logcat -s diari
```

### Android build notes
`scripts/android.sh` sets up everything the cross-build needs:
- **NDK r30 or newer.** Slint's Skia renderer ships prebuilt binaries made with NDK r30's toolchain, and they need its libc++. `build.rs` links that libc++ statically.
- **Newest installed build-tools.** The `d8` in build-tools 34 crashes on Slint's Java helper.
- **`llvm-ranlib` for vendored OpenSSL.**
- **Debug-keystore signing**, unless `CARGO_APK_RELEASE_KEYSTORE` is set.
