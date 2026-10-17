//! cargo run --no-default-features --example system_icons_probe
//! Emits a contact sheet using the application's existing PNG encoder.
use anyhow::Result;
use rust_explorer::system_icons::{IconBitmap, file_icon, ui_icon};
use std::{path::Path, time::Instant};

fn write_sheet(path: &Path, icons: &[IconBitmap]) -> Result<()> {
    let width = 512usize;
    let height = icons.len().div_ceil(8) * 64;
    let mut rgb = vec![240u8; width * height * 3];
    for (i, icon) in icons.iter().enumerate() {
        for y in 0..icon.height as usize {
            for x in 0..icon.width as usize {
                let src = (y * icon.width as usize + x) * 4;
                let dst = ((i / 8 * 64 + y) * width + i % 8 * 64 + x) * 3;
                let a = u32::from(icon.rgba[src + 3]);
                for c in 0..3 {
                    rgb[dst + c] =
                        ((u32::from(icon.rgba[src + c]) * a + 240 * (255 - a)) / 255) as u8;
                }
            }
        }
    }
    image::save_buffer_with_format(
        path,
        &rgb,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
        image::ImageFormat::Png,
    )?;
    Ok(())
}

fn main() -> Result<()> {
    std::fs::create_dir_all("artifacts/system-icons")?;
    let names = [
        "up",
        "refresh",
        "new-folder",
        "cut",
        "copy",
        "paste",
        "rename",
        "delete",
        "preview",
        "search",
        "home",
        "desktop",
        "downloads",
        "documents",
        "pictures",
        "music",
        "videos",
        "drive",
        "close",
        "video",
        "folder",
    ];
    let mut icons = Vec::new();
    for name in names {
        let start = Instant::now();
        match ui_icon(name, 32) {
            Ok(icon) => {
                let transparent = icon.rgba.chunks_exact(4).filter(|p| p[3] == 0).count();
                assert!(
                    icon.rgba.chunks_exact(4).any(|p| p[3] > 0),
                    "empty icon: {name}"
                );
                assert!(transparent > 0, "no transparent background: {name}");
                println!(
                    "{name}: {}x{}, {transparent} transparent pixels, {:?}",
                    icon.width,
                    icon.height,
                    start.elapsed()
                );
                icons.push(icon);
            }
            Err(err) => anyhow::bail!("{name}: {err:#}"),
        }
    }
    for file in [
        "file.mp4",
        "file.txt",
        "file.pdf",
        "file.png",
        "file.unknownextension",
        "file.exe",
    ] {
        let start = Instant::now();
        icons.push(file_icon(Path::new(file), false, 32)?);
        println!("{file}: {:?}", start.elapsed());
    }
    assert!(ui_icon("back", 32).is_err());
    assert!(ui_icon("tabs", 32).is_err());
    assert!(ui_icon("copy", u32::MAX).is_err());
    write_sheet(
        Path::new("artifacts/system-icons/helper-output.png"),
        &icons,
    )?;
    Ok(())
}
