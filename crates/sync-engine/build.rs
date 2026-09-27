use std::{env, path::PathBuf, process::Command};

fn main() {
    let source =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../browser-cookies");
    for file in ["src", "Cargo.toml", "Cargo.lock"] {
        println!("cargo:rerun-if-changed={}", source.join(file).display());
    }
    let target = env::var("TARGET").unwrap();
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let build = output.join("cookie-reader-build");
    let status = Command::new(env::var_os("CARGO").unwrap())
        .args(["build", "--locked", "--release", "--manifest-path"])
        .arg(source.join("Cargo.toml"))
        .args(["--target", &target, "--target-dir"])
        .arg(&build)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_MAKEFLAGS")
        .status()
        .expect("build native Rust cookie reader");
    assert!(status.success(), "native Rust cookie reader build failed");
    let name = if target.contains("windows") {
        "listenbox-browser-cookies.exe"
    } else {
        "listenbox-browser-cookies"
    };
    std::fs::copy(
        build.join(target).join("release").join(name),
        output.join("browser-cookies"),
    )
    .expect("bundle native Rust cookie reader");
}
