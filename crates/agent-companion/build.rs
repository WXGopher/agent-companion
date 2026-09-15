//! Compile only the UI used by the target platform.
fn main() {
    let entry = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        "ui/agent-companion.slint"
    } else {
        "ui/codex-tui.slint"
    };
    slint_build::compile(entry).expect("the Slint markup failed to compile");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        native_macos();
    }
}

fn native_macos() {
    use std::path::PathBuf;
    use std::process::Command;
    let xcrun = |args: &[&str]| {
        let result = Command::new("xcrun")
            .args(args)
            .output()
            .expect("Xcode command line tools are required");
        assert!(
            result.status.success(),
            "xcrun failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout)
            .expect("Xcode path is UTF-8")
            .trim()
            .to_owned()
    };
    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let compiler = xcrun(&["--find", "swiftc"]);
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let mut sources: Vec<_> = std::fs::read_dir("macos")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "swift")
        })
        .collect();
    sources.sort();
    println!("cargo:rerun-if-changed=macos");
    let result = Command::new(&compiler)
        .args([
            "-emit-library",
            "-static",
            "-module-name",
            "AgentCompanionMac",
            "-swift-version",
            "5",
            "-O",
            "-whole-module-optimization",
            "-target",
        ])
        .arg(format!("{arch}-apple-macosx14.0"))
        .args(["-sdk", &sdk])
        .args(&sources)
        .arg("-o")
        .arg(output.join("libagentcompanionmac.a"))
        .status()
        .expect("could not run the Swift compiler");
    assert!(result.success(), "the native macOS shell failed to compile");
    let toolchain = PathBuf::from(compiler)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain.join("lib/swift/macosx").display()
    );
    println!("cargo:rustc-link-lib=static=agentcompanionmac");
    println!("cargo:rustc-link-lib=framework=SwiftUI");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    // Keep the executable's Mach-O minimum aligned with the native Swift layer
    // and the packaged Info.plist, including direct command-line launches.
    println!("cargo:rustc-link-arg=-mmacosx-version-min=14.0");
}
