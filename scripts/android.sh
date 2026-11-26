#!/usr/bin/env bash
# Build (or run) the Android APK.
#   scripts/android.sh build            # aarch64 device build
#   scripts/android.sh run              # build + install + launch on connected device
#   TARGET=x86_64-linux-android scripts/android.sh run   # emulator
set -euo pipefail
cd "$(dirname "$0")/.."

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
if [[ -z "${ANDROID_NDK_ROOT:-}" ]]; then
  ANDROID_NDK_ROOT="$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)"
fi
export ANDROID_NDK_ROOT
TARGET="${TARGET:-aarch64-linux-android}"
CMD="${1:-build}"

HOST_TAG="darwin-x86_64"; [[ "$(uname)" == Linux ]] && HOST_TAG="linux-x86_64"
NDK_BIN="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/$HOST_TAG/bin"

# Vendored OpenSSL looks for <triple>-ranlib, which modern NDKs no longer ship.
export "RANLIB_aarch64_linux_android=$NDK_BIN/llvm-ranlib"
export "RANLIB_x86_64_linux_android=$NDK_BIN/llvm-ranlib"
export "AR_aarch64_linux_android=$NDK_BIN/llvm-ar"
export "AR_x86_64_linux_android=$NDK_BIN/llvm-ar"
export PATH="$NDK_BIN:$PATH"

# Slint compiles a Java helper and dexes it with d8. The d8 in build-tools 34
# crashes on release-mode classes, so use the newest installed build-tools.
if [[ -z "${ANDROID_BUILD_TOOLS_VERSION:-}" ]]; then
  ANDROID_BUILD_TOOLS_VERSION="$(ls "$ANDROID_HOME/build-tools" | sort -V | tail -1)"
fi
export ANDROID_BUILD_TOOLS_VERSION

# Sign release builds with the debug keystore unless a real one is configured.
if [[ -z "${CARGO_APK_RELEASE_KEYSTORE:-}" ]]; then
  KS="$HOME/.android/debug.keystore"
  if [[ ! -f "$KS" ]]; then
    mkdir -p "$(dirname "$KS")"
    keytool -genkeypair -keystore "$KS" -storepass android -alias androiddebugkey \
      -keypass android -keyalg RSA -keysize 2048 -validity 10000 \
      -dname "CN=Android Debug,O=Android,C=US"
  fi
  export CARGO_APK_RELEASE_KEYSTORE="$KS" CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android
fi

cargo apk "$CMD" --lib --release --target "$TARGET"
