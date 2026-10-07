fn main() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let prefix = std::path::PathBuf::from(
        std::env::var_os("FFMPEG_DIR").expect("bundled FFMPEG_DIR is required"),
    );
    // Cargo otherwise reuses a statically linked executable after Moon rebuilds
    // its C libraries at the same path, leaving the old encoder in the binary.
    let libraries = prefix.join("lib");
    println!("cargo:rerun-if-changed={}", libraries.display());
    let mut archives = std::fs::read_dir(&libraries)
        .expect("native libraries are prepared")
        .map(|entry| entry.expect("read native library").path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("a" | "lib")
            )
        })
        .collect::<Vec<_>>();
    archives.sort();
    for path in archives {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // The bundled FFmpeg encoder links the source-built OpenH264 archive.
    println!("cargo:rustc-link-lib=static=openh264");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=stdc++");
    } else if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=c++");
    }
}
