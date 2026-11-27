fn main() {
    // Debug builds keep element info so tests/menu.rs can find and click
    // the UI's buttons.
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");

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
        // whisper-rs-sys asks for the shared libc++. A linker script by that
        // name, found here before the NDK's, points it at the static one too.
        std::fs::write(format!("{out}/libc++_shared.so"), "INPUT(-lc++_static -lc++abi)\n")
            .expect("write libc++_shared.so linker script");
        println!("cargo:rustc-link-search=native={out}");
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
    }
}
