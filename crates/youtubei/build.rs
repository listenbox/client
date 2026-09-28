use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let bundle = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../vendor/youtubejs/bundle/cf-worker.js");
    println!("cargo:rerun-if-changed={}", bundle.display());
    let source = fs::read(&bundle).map_err(|_| {
        "YouTube.js source bundle is missing. Initialize submodules recursively and run moon run youtubejs:bundle before Cargo."
    })?;
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo must set OUT_DIR")?).join("youtubei.js");
    fs::write(output, source)?;
    Ok(())
}
