use image::{
    ExtendedColorType,
    codecs::ico::{IcoEncoder, IcoFrame},
    imageops::FilterType,
};
use resvg::{tiny_skia, usvg};
use std::{env, error::Error, fs, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rustc-check-cfg=cfg(listenbox_updater)");
    println!("cargo:rerun-if-env-changed=LISTENBOX_UPDATE_PUBLIC_KEY");
    println!("cargo:rerun-if-changed=src/updater/macos.m");
    println!("cargo:rerun-if-env-changed=LISTENBOX_SOURCE_COMMIT");
    let commit = env::var("LISTENBOX_SOURCE_COMMIT").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .unwrap_or_else(|| "development".into())
            .trim()
            .to_owned()
    });
    println!("cargo:rustc-env=LISTENBOX_SOURCE_COMMIT={commit}");
    if env::var_os("CARGO_FEATURE_NATIVE_UPDATER").is_some()
        && env::var("PROFILE")? == "release"
        && matches!(
            env::var("CARGO_CFG_TARGET_OS")?.as_str(),
            "macos" | "windows"
        )
    {
        let key = env::var("LISTENBOX_UPDATE_PUBLIC_KEY")?;
        if key.len() != 44
            || !key.ends_with('=')
            || !key[..43]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(&b))
        {
            return Err("A base64-encoded 32-byte Ed25519 public key is required".into());
        }
        println!("cargo:rustc-cfg=listenbox_updater");
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("../../.cache/updaters");
        let platform = if env::var("CARGO_CFG_TARGET_OS")? == "macos" {
            cc::Build::new()
                .file("src/updater/macos.m")
                .flag("-fobjc-arc")
                .flag("-fblocks")
                .flag(format!("-F{}", root.display()))
                .compile("listenbox-updater");
            println!("cargo:rustc-link-search=framework={}", root.display());
            println!("cargo:rustc-link-lib=framework=Sparkle");
            println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
            "macos-arm64"
        } else {
            let arm = env::var("CARGO_CFG_TARGET_ARCH")? == "aarch64";
            let architecture = if arm { "ARM64" } else { "x64" };
            println!(
                "cargo:rustc-link-search=native={}",
                root.join("WinSparkle-0.9.4")
                    .join(architecture)
                    .join("Release")
                    .display()
            );
            if arm { "windows-arm64" } else { "windows-x64" }
        };
        println!("cargo:rustc-env=LISTENBOX_UPDATE_PLATFORM={platform}");
        println!("cargo:rustc-env=LISTENBOX_UPDATE_PUBLIC_KEY={key}");
    }
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/tray.svg");
    if cfg!(feature = "hot-reload") && env::var("CARGO_CFG_TARGET_OS")? == "macos" {
        // Subsecond's fat binary includes GPUI's otherwise unused IOSurface code.
        println!("cargo:rustc-link-lib=framework=IOSurface");
    }
    let out = std::path::PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);

    // Generate both variants on every host so they can be inspected together.
    // tray-icon displays macOS images at 18pt: 36px supplies the Retina bitmap.
    render_tray(&out, "tray-macos", 36, "path, rect { fill: black; }")?;
    // A narrow charcoal keyline keeps the white mark legible on light taskbars.
    render_tray(
        &out,
        "tray-windows",
        32,
        "path, rect { fill: #FFFEFA; stroke: #151514; stroke-width: 10; stroke-linejoin: round; paint-order: stroke fill; }",
    )?;
    app_icon(&out)?;

    if env::var("CARGO_CFG_TARGET_OS")? == "windows" {
        // GPUI loads resource 1 for its window/taskbar icon. Explorer uses the
        // same resource from the executable, including standalone downloads.
        let resource = out.join("icon.rc");
        let version = env::var("CARGO_PKG_VERSION")?;
        let numeric = format!("{},0", version.replace('.', ","));
        fs::write(
            &resource,
            format!(
                r#"1 ICON "icon.ico"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004L
FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904B0"
  BEGIN
   VALUE "CompanyName", "Listenbox"
   VALUE "ProductName", "Listenbox"
   VALUE "ProductVersion", "{version}"
   VALUE "FileVersion", "{version}.0"
   VALUE "Comments", "Source commit: {commit}"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#
            ),
        )?;
        embed_resource::compile_for(
            &resource,
            ["listenbox-desktop"],
            embed_resource::ParamsIncludeDirs([out.as_path()]),
        )
        .manifest_required()?;
    }
    Ok(())
}

fn app_icon(out: &Path) -> Result<(), Box<dyn Error>> {
    // The macOS bundle's artwork is the sole source for Windows as well.
    let source = image::open("assets/icon.png")?;
    let mut frames = Vec::new();
    for size in [16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256] {
        let pixels = source
            .resize_exact(size, size, FilterType::Lanczos3)
            .to_rgba8();
        frames.push(IcoFrame::as_png(
            &pixels,
            size,
            size,
            ExtendedColorType::Rgba8,
        )?);
    }
    IcoEncoder::new(fs::File::create(out.join("icon.ico"))?).encode_images(&frames)?;
    Ok(())
}

fn render_tray(out: &Path, name: &str, size: u32, style: &str) -> Result<(), Box<dyn Error>> {
    let options = usvg::Options {
        style_sheet: Some(style.into()),
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(&fs::read("assets/tray.svg")?, &options)?;
    let mut pixels = tiny_skia::Pixmap::new(size, size).ok_or("invalid tray dimensions")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            size as f32 / tree.size().width(),
            size as f32 / tree.size().height(),
        ),
        &mut pixels.as_mut(),
    );
    // tray-icon expects straight RGBA, while tiny-skia renders premultiplied RGBA.
    let rgba: Vec<u8> = pixels
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let pixel = pixel.demultiply();
            [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
        })
        .collect();
    fs::write(out.join(format!("{name}.rgba")), &rgba)?;
    // Keep inspectable previews alongside the embedded build artifacts.
    image::save_buffer(
        out.join(format!("{name}.png")),
        &rgba,
        size,
        size,
        image::ColorType::Rgba8,
    )?;
    Ok(())
}
