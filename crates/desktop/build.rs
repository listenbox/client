use image::{
    ExtendedColorType,
    codecs::ico::{IcoEncoder, IcoFrame},
    imageops::FilterType,
};
use resvg::{tiny_skia, usvg};
use std::{env, error::Error, fs, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/tray.svg");
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
        fs::write(&resource, "1 ICON \"icon.ico\"\n")?;
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
