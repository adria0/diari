fn main() {
    slint_build::compile("ui/app.slint").expect("failed to compile Slint UI");

    // Some native dependencies use the C++ standard library. cargo-apk does not
    // package libc++_shared.so, so link the NDK's static libc++ into libdiari.so.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rerun-if-env-changed=ANDROID_NDK_ROOT");
        let ndk = std::env::var("ANDROID_NDK_ROOT")
            .or_else(|_| std::env::var("ANDROID_NDK_HOME"))
            .expect("set ANDROID_NDK_ROOT (scripts/android.sh does this)");
        let triple = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "aarch64-linux-android",
            "x86_64" => "x86_64-linux-android",
            "arm" => "arm-linux-androideabi",
            "x86" => "i686-linux-android",
            other => panic!("unsupported Android arch {other}"),
        };
        let host = if cfg!(target_os = "macos") { "darwin-x86_64" } else { "linux-x86_64" };
        // Copy just the libc++ archives: adding the whole sysroot lib dir to the
        // search path would make `-lz` pick the static libz.a and clash with Skia.
        let src = format!("{ndk}/toolchains/llvm/prebuilt/{host}/sysroot/usr/lib/{triple}");
        let out = std::env::var("OUT_DIR").unwrap();
        for lib in ["libc++_static.a", "libc++abi.a"] {
            std::fs::copy(format!("{src}/{lib}"), format!("{out}/{lib}"))
                .unwrap_or_else(|e| panic!("copy {src}/{lib}: {e}"));
        }
        println!("cargo:rustc-link-search=native={out}");
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
    }
}
