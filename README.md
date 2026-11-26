# perb

Native markdown notes with automatic git sync, for macOS and Android. It's written in Rust, and the UI is Slint (no webview).

## Features
- Lists every `*.md` file in the notes folder, including subfolders. You can search, create, rename and delete notes.
- Plain-text editor that autosaves 1 s after you stop typing, plus a native markdown preview.
- Git sync over SSH, using an ed25519 key the app generates. Sync runs:
  - on startup, on Android resume, and when you tap the status chip
  - 5 s after a save
  - every 5 min
- **Conflicts never lose data.** If two devices edit the same note, the remote version keeps the name and the local version is saved as `name.conflict-<device>-<timestamp>.md`.

## Where things live
| | macOS | Android |
|---|---|---|
| Config, SSH key, known hosts | `~/Library/Application Support/perb/` | app-private storage |
| Notes | `~/Documents/perb/` (configurable) | app-private `notes/` |

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
sdkmanager "ndk;27.2.12479018"
cargo install cargo-apk
export ANDROID_HOME=~/Library/Android/sdk
export ANDROID_NDK_ROOT=$ANDROID_HOME/ndk/27.2.12479018

cargo apk run --lib --release                                 # device (aarch64)
cargo apk run --lib --release --target x86_64-linux-android   # emulator
adb logcat -s perb
```
