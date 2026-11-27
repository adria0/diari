# diari

Native markdown notes with automatic git sync, for macOS and Android. It's written in Rust, and the UI is Slint (no webview).

<p>
  <img src="docs/screenshots/macos.png" alt="diari on macOS" height="420">
</p>

## Features
- Lists every `*.md` file in the notes folder, including subfolders. You can search, create, rename and delete notes. Delete (in a note's ⋮ menu) and `CONFIG.md` only show with **Advanced mode** on (Settings).
- Plain-text editor that autosaves 1 s after you stop typing, plus a native markdown preview.
- Git sync over SSH, using an ed25519 key the app generates. Sync runs:
  - on startup, on Android resume, and when you tap the status chip
  - 1 min after the last save (each new save restarts the minute)
  - every 5 min
- **Conflicts never lose data.** If two devices edit the same note, the remote version keeps the name and the local version is saved as `name.conflict-<device>-<timestamp>.md`.
- **Files and images (Android).** Share files or images to diari from any app, one or several at once. They're saved in today's note's attachment folder (`2026_11_26.md.files/`), and each one adds a line like `- 09:01 [photo.jpg](2026_11_26.md.files/photo.jpg)`.
  - In the preview, pictures are shown (local ones, and `![](https://…)` ones from the web, downloaded once into a cache), upright and scaled down; task items (`- [ ]`) are checkboxes you can tick, and other links open when you tap them: web links in the browser, files in the app for their type. On Android, a file is first copied to `Download/diari/` so other apps can read it (Android 10 or later).
  - Attachment folders aren't shown in the note list, are synced with git, move when their note is renamed, and are deleted with it.
  - The Files entry in a note's ⋮ menu lists its files (with thumbnails for images) to open or delete them. Deleting a file also removes the note's line that linked to it.
- **Workspaces.** Top-level folders with their own `CONFIG.md`. `diari/` is the journal, where daily notes are made (and voice notes, photos and shares go); others (`feina/`, `familia/`…) hold notes on a subject. The title of the note list switches between them (or shows all). Older repos are offered to move their notes into `diari/`.
- **Links between notes.** `[[Name]]` (or `[[Name|text]]`) links to the note `Name.md`; tap it in the preview to open it, or to create it if it doesn't exist. At the end of each note's preview, *Linked from* lists the notes that link to it, with the line that does. Renaming a note updates the links to it. While typing `[[`, the editor offers note names to finish the link with a tap, and under the editor *Linked from:* shows the notes that link to the open one.
- **Voice notes.** The 🎙 button (next to Today, or *Voice note* in a note's ⋮ menu) records you, transcribes it on the device with [Whisper](https://github.com/ggml-org/whisper.cpp), and adds the text as `- HH:MM text` to today's note (or the open one). The audio isn't kept.
  - The speech model (Whisper small, 190 MB) is downloaded once, the first time you use it.
  - The language is set in Settings (`ca` by default; `auto` detects it).
  - Recordings stop on their own after 10 minutes.
- **Calendars.** When today's note is created, diari adds the day's events from the iCal feeds (Proton Calendar share links, for example) listed in `CONFIG.md` at the top of the notes folder, under a `# cal` heading:

  ````markdown
  # cal
  ```yaml
  - name: Personal
    icalurl: https://calendar.proton.me/api/calendar/v1/url/…/calendar.ics?…
  - name: Tasks
    icalurl: https://…
    is_todo: true
  ```
  ````

  `url` works in place of `icalurl`, the list may be wrapped in `calendars:`, and other fields (`icon`, `desc`…) are ignored, so the same list can be shared with other tools.

  With `CONFIG.md` open, **Check config** in the ⋮ menu shows any problem in it, loads each calendar, and shows what today's note would get.

  Plain calendars go under `## Calendar` (today's events). Calendars with `is_todo: true` go under `## To do`, with every event from the past month up to today as a `- [ ]` item. The feeds are fetched in the background, and repeating events are included. `CONFIG.md` is synced like any note, so the calendar links (which work as passwords) are stored in your notes repo.

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
brew install cmake        # whisper.cpp, for voice notes
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
