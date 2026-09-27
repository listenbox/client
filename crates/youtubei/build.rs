use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::{env, error::Error, fs, io::Read, path::PathBuf, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest: toml::Value = toml::from_str(&fs::read_to_string("Cargo.toml")?)?;
    let upstream = &manifest["package"]["metadata"]["youtubei"];
    let field = |name: &str| {
        upstream[name]
            .as_str()
            .expect("invalid youtubei package metadata")
    };
    let output =
        PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo must set OUT_DIR")?).join("youtubei.js");
    println!("cargo:rerun-if-changed={}", output.display());
    if fs::read(&output).is_ok_and(|bytes| checksum(&bytes) == field("bundle-sha256")) {
        return Ok(());
    }

    let url = format!(
        "https://registry.npmjs.org/youtubei.js/-/youtubei.js-{}.tgz",
        field("version")
    );
    let mut archive = Vec::new();
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(url)
        .send()?
        .error_for_status()?
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut archive)?;
    if checksum(&archive) != field("archive-sha256") {
        return Err("youtubei.js archive SHA-256 mismatch".into());
    }
    let mut archive = tar::Archive::new(GzDecoder::new(archive.as_slice()));
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.path()?.as_ref() != std::path::Path::new("package/bundle/cf-worker.js") {
            continue;
        }
        let mut bundle = Vec::new();
        entry.take(8 * 1024 * 1024 + 1).read_to_end(&mut bundle)?;
        if checksum(&bundle) != field("bundle-sha256") {
            return Err("youtubei.js CF-worker bundle SHA-256 mismatch".into());
        }
        // Write only the verified bundle; never unpack archive paths onto disk.
        fs::write(output, bundle)?;
        return Ok(());
    }
    Err("youtubei.js archive is missing package/bundle/cf-worker.js".into())
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
