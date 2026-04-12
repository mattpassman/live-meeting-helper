fn main() {
    #[cfg(feature = "app")]
    tauri_build::build();

    compile_macos_loopback();
}

/// Compile loopback_mac.swift → static lib and set linker flags.
/// No-ops on non-macOS targets.
fn compile_macos_loopback() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();

    let swift_src = format!("{}/src/audio/loopback_mac.swift", manifest_dir);
    let obj_out = format!("{}/loopback_mac.o", out_dir);
    let lib_out = format!("{}/libloopback_mac.a", out_dir);

    // macOS SDK path
    let sdk_out = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("xcrun not found — install Xcode Command Line Tools");
    let sdk = String::from_utf8(sdk_out.stdout).unwrap().trim().to_string();

    // Target triple: match Rust's target architecture
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "aarch64".into());
    let swift_arch = if arch == "aarch64" { "arm64" } else { "x86_64" };
    // Target macOS 14.4 — required for CATapDescription / AudioHardwareCreateProcessTap.
    // The @_cdecl functions guard with #available(macOS 14.4, *) so the binary
    // loads on older macOS and returns a clear error instead of crashing.
    let swift_target = format!("{}-apple-macos14.4", swift_arch);

    // Compile Swift → object file
    let status = std::process::Command::new("swiftc")
        .args([
            "-sdk", &sdk,
            "-target", &swift_target,
            "-O",
            "-parse-as-library",
            "-emit-object",
            "-o", &obj_out,
            &swift_src,
        ])
        .status()
        .expect("swiftc not found — install Xcode to enable macOS system audio capture");
    assert!(status.success(), "Swift compilation of loopback_mac.swift failed");

    // Archive into a static library Cargo can link
    let status = std::process::Command::new("ar")
        .args(["rcs", &lib_out, &obj_out])
        .status()
        .expect("ar not found");
    assert!(status.success(), "ar failed creating libloopback_mac.a");

    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=loopback_mac");

    // CoreAudio tap APIs (always present on macOS, new APIs added in 14.4)
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Foundation");

    // Swift runtime (embedded in macOS 12+)
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=dylib=swiftFoundation");
    println!("cargo:rustc-link-lib=dylib=swiftObjectiveC");

    println!("cargo:rerun-if-changed=src/audio/loopback_mac.swift");
}
