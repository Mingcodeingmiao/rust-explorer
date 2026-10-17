//! Run with: cargo run --no-default-features --example native_preview_probe -- <file> [page]
use anyhow::{Context, Result};
use rust_explorer::native_preview;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().context("请指定视频或 PDF 文件")?);
    let page = args
        .next()
        .and_then(|p| p.to_str().and_then(|p| p.parse().ok()))
        .unwrap_or(0);
    let start = Instant::now();
    let (bitmap, pages) = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        native_preview::pdf_page(&path, page, 640)?
    } else {
        (native_preview::video_thumbnail(&path, 640)?, 1)
    };
    println!(
        "{}x{} RGBA bytes={} pages={} elapsed_ms={}",
        bitmap.width,
        bitmap.height,
        bitmap.rgba.len(),
        pages,
        start.elapsed().as_millis()
    );
    Ok(())
}
