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
    let production = env::var("LISTENBOX_PRODUCTION_RELEASE").as_deref() == Ok("1");
    let updater_key = if production {
        env::var("LISTENBOX_UPDATE_PUBLIC_KEY")?
    } else {
        String::new()
    };
    let updater_metadata = if production {
        format!(
            r#"<key>SUFeedURL</key><string>https://github.com/listenbox/client/releases/latest/download/appcast-macos-arm64.xml</string>
<key>SUPublicEDKey</key><string>{updater_key}</string>
<key>SUEnableAutomaticChecks</key><true/>
<key>SUAutomaticallyUpdate</key><false/>
<key>SUUpdateCheckInterval</key><integer>86400</integer>"#
        )
    } else {
        String::new()
    };
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
    if production {
        fs::create_dir_all(contents.join("Frameworks"))?;
        run(Command::new("ditto")
            .arg(".cache/updaters/Sparkle.framework")
            .arg(contents.join("Frameworks/Sparkle.framework")))?;
        fs::copy(
            ".cache/updaters/LICENSE",
            contents.join("Resources/Sparkle-LICENSE.txt"),
        )?;
    }
    let mut provenance = Command::new(contents.join("MacOS/listenbox-desktop"));
    provenance.arg("--version");
    let output = provenance.output()?;
    let expected = format!(
        "Listenbox {version} ({})",
        env::var("LISTENBOX_SOURCE_COMMIT").unwrap_or_default()
    );
    if production
        && (!output.status.success() || String::from_utf8(output.stdout)?.trim() != expected)
    {
        return Err("Packaged executable disagrees with release version/source commit".into());
    }
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
{updater_metadata}
<key>ListenboxSourceCommit</key><string>{}</string>
</dict></plist>"#,
            env::var("LISTENBOX_SOURCE_COMMIT").unwrap_or_default()
        ),
    )?;
    run(Command::new("plutil")
        .arg("-lint")
        .arg(contents.join("Info.plist")))?;
    // Ad-hoc signing makes local arm64 bundles executable. Distribution signing
    // is explicit: a Developer ID identity may be supplied by the release job.
    let identity = env::var("LISTENBOX_SIGNING_IDENTITY").unwrap_or_else(|_| "-".into());
    if production && !identity.starts_with("Developer ID Application: ") {
        return Err("Production packaging requires a Developer ID Application identity".into());
    }
    let app = staging.join("Listenbox.app");
    if production {
        let framework = contents.join("Frameworks/Sparkle.framework");
        for nested in [
            "Versions/B/XPCServices/Installer.xpc",
            "Versions/B/XPCServices/Downloader.xpc",
            "Versions/B/Autoupdate",
            "Versions/B/Updater.app",
            "",
        ] {
            let target = framework.join(nested);
            let mut command = Command::new("codesign");
            command.args([
                "--force",
                "--sign",
                &identity,
                "--options",
                "runtime",
                "--timestamp",
            ]);
            if nested.contains("Downloader.xpc") {
                command.arg("--preserve-metadata=entitlements");
            }
            run(command.arg(target))?;
        }
    }
    let mut signing = Command::new("codesign");
    signing.args(["--force", "--sign", &identity]);
    if identity != "-" {
        signing.args(["--options", "runtime", "--timestamp"]);
    }
    run(signing.arg(&app))?;
    run(Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&app))?;
    if production {
        let zip = dist.join("notarization.zip");
        run(Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&app)
            .arg(&zip))?;
        notarize(&zip)?;
        fs::remove_file(zip)?;
        run(Command::new("xcrun").args(["stapler", "staple"]).arg(&app))?;
        run(Command::new("xcrun")
            .args(["stapler", "validate"])
            .arg(&app))?;
        run(Command::new("spctl")
            .args(["--assess", "--type", "execute", "--verbose=2"])
            .arg(&app))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))?;
    let dmg = dist.join(format!("Listenbox-{version}-macos-arm64.dmg"));
    run(Command::new("hdiutil")
        .args(["create", "-volname", "Listenbox", "-srcfolder"])
        .arg(&staging)
        .args(["-ov", "-format", "UDZO"])
        .arg(&dmg))?;
    run(Command::new("hdiutil").arg("verify").arg(&dmg))?;
    if production {
        run(Command::new("codesign")
            .args(["--force", "--sign", &identity, "--timestamp"])
            .arg(&dmg))?;
        notarize(&dmg)?;
        run(Command::new("xcrun").args(["stapler", "staple"]).arg(&dmg))?;
        run(Command::new("xcrun")
            .args(["stapler", "validate"])
            .arg(&dmg))?;
        run(Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&dmg))?;
    }
    println!("{}", dmg.display());
    Ok(())
}

fn notarize(path: &Path) -> Result<(), Box<dyn Error>> {
    let output = Command::new("xcrun")
        .args(["notarytool", "submit"])
        .arg(path)
        .arg("--key")
        .arg(env::var("LISTENBOX_NOTARY_KEY")?)
        .arg("--key-id")
        .arg(env::var("MACOS_NOTARY_KEY_ID")?)
        .arg("--issuer")
        .arg(env::var("MACOS_NOTARY_ISSUER_ID")?)
        .args(["--wait", "--output-format", "json"])
        .output()?;
    let receipt = path.with_extension("notary.json");
    fs::write(&receipt, &output.stdout)?;
    if !output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Err(format!(
            "Notarization submission failed; receipt: {}",
            receipt.display()
        )
        .into());
    }
    let status = Command::new("plutil")
        .args(["-extract", "status", "raw", "-o", "-"])
        .arg(&receipt)
        .output()?;
    if !status.status.success() || String::from_utf8(status.stdout)?.trim() != "Accepted" {
        return Err(format!(
            "Apple did not accept notarization; receipt: {}",
            receipt.display()
        )
        .into());
    }
    println!("Notarization accepted: {}", path.display());
    Ok(())
}
