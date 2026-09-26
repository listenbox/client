//! Build a relocatable application bundle and DMG. No build-machine config is packaged.
use std::{env, error::Error, fs, path::Path, process::Command};

fn run(command: &mut Command) -> Result<(), Box<dyn Error>> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{command:?} failed: {status}").into());
    }
    Ok(())
}

fn require_arm64(path: &Path) -> Result<(), Box<dyn Error>> {
    let output = Command::new("lipo").arg("-archs").arg(path).output()?;
    if !output.status.success() {
        return Err(format!("cannot inspect architectures in {}", path.display()).into());
    }
    let actual = String::from_utf8(output.stdout)?;
    if actual.trim() != "arm64" {
        return Err(format!("{} must be arm64, found {}", path.display(), actual.trim()).into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    if !cfg!(target_os = "macos") {
        return Err("DMGs require macOS".into());
    }
    let manifest = fs::read_to_string("Cargo.toml")?;
    let version = manifest
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .ok_or("missing workspace version")?;
    if !version
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err("invalid version".into());
    }
    let dist = Path::new("crates/desktop/dist/macos");
    let binary = Path::new("crates/desktop/dist/release/listenbox-desktop");
    require_arm64(binary)?;
    let staging = dist.join("image");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let contents = staging.join("Listenbox.app/Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    fs::copy(binary, contents.join("MacOS/listenbox-desktop"))?;
    fs::copy(
        "crates/desktop/assets/icon.png",
        contents.join("Resources/icon.png"),
    )?;
    for (source, target) in [
        ("LICENSE", "LICENSE"),
        ("THIRD-PARTY-NOTICES.txt", "THIRD-PARTY-NOTICES.txt"),
    ] {
        fs::copy(source, contents.join("Resources").join(target))?;
    }
    fs::copy(
        "crates/desktop/dist/release/FFmpeg-NOTICE.txt",
        contents.join("Resources/FFmpeg-NOTICE.txt"),
    )?;
    fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Listenbox</string>
<key>CFBundleDisplayName</key><string>Listenbox</string>
<key>CFBundleIdentifier</key><string>app.listenbox.client</string>
<key>CFBundleExecutable</key><string>listenbox-desktop</string>
<key>CFBundleIconFile</key><string>icon.png</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>{version}</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>"#
        ),
    )?;
    run(Command::new("plutil")
        .arg("-lint")
        .arg(contents.join("Info.plist")))?;
    // Ad-hoc signing makes local arm64 bundles executable. Distribution signing
    // is explicit: a Developer ID identity may be supplied by the release job.
    let identity = env::var("LISTENBOX_SIGNING_IDENTITY").unwrap_or_else(|_| "-".into());
    let app = staging.join("Listenbox.app");
    let mut signing = Command::new("codesign");
    signing.args(["--force", "--sign", &identity]);
    if identity != "-" {
        signing.args(["--options", "runtime", "--timestamp"]);
    }
    run(signing.arg(&app))?;
    run(Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(&app))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    let dmg = dist.join(format!("Listenbox-{version}-macos-arm64.dmg"));
    run(Command::new("hdiutil")
        .args(["create", "-volname", "Listenbox", "-srcfolder"])
        .arg(&staging)
        .args(["-ov", "-format", "UDZO"])
        .arg(&dmg))?;
    run(Command::new("hdiutil").arg("verify").arg(&dmg))?;
    println!("{}", dmg.display());
    Ok(())
}
