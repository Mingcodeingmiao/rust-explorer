//! Exercises the exact helper IPC, deadlines and PNG caches used by the GUI.
use anyhow::{Result, ensure};
use rust_explorer::native_assets::{self, IconRequest, PreviewKind};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--native-asset") {
        let json = args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("request missing"))?;
        if json.contains("__simulate_slow_provider__") {
            std::thread::sleep(Duration::from_secs(5));
            return Ok(());
        }
        return native_assets::serve(json);
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = root.join(".tools/preview-fixtures");
    let cancel = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    let video = native_assets::preview(
        &fixtures.join("flower.mp4"),
        PreviewKind::Video,
        0,
        640,
        &cancel,
    )?;
    let video_time = start.elapsed();
    let png = image::load_from_memory(&video.image)?;
    ensure!(
        png.width() > 100 && png.height() > 100,
        "video frame has no pixels"
    );
    let start = Instant::now();
    let cached = native_assets::preview(
        &fixtures.join("flower.mp4"),
        PreviewKind::Video,
        0,
        640,
        &cancel,
    )?;
    ensure!(cached.image == video.image, "thumbnail cache miss");
    let cache_time = start.elapsed();
    let pdf = native_assets::preview(
        &fixtures.join("sample.pdf"),
        PreviewKind::Pdf,
        0,
        960,
        &cancel,
    )?;
    ensure!(
        pdf.pages == 1 && image::load_from_memory(&pdf.image)?.width() > 100,
        "native PDF image invalid"
    );
    let icons = native_assets::icons(vec![
        IconRequest::ui("cut", 24),
        IconRequest::file(&PathBuf::from("file.mp4"), false, 24),
        IconRequest::ui("__no_matching_resource__", 24),
    ]);
    ensure!(
        icons.len() == 3 && icons[0].is_some() && icons[1].is_some() && icons[2].is_none(),
        "native icon/fallback result invalid"
    );
    let start = Instant::now();
    let slow = native_assets::preview(
        &PathBuf::from("__simulate_slow_provider__"),
        PreviewKind::Video,
        0,
        640,
        &cancel,
    );
    ensure!(
        slow.is_err() && start.elapsed() < Duration::from_millis(2500),
        "provider deadline failed"
    );
    let timeout_time = start.elapsed();
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        signal.store(true, Ordering::Relaxed);
    });
    let start = Instant::now();
    let cancelled = native_assets::preview(
        &PathBuf::from("__simulate_slow_provider__"),
        PreviewKind::Video,
        0,
        640,
        &cancel,
    );
    trigger.join().unwrap();
    ensure!(
        cancelled.is_err() && start.elapsed() < Duration::from_millis(700),
        "preview cancellation failed"
    );
    println!(
        "PASS: native video frame {}x{}, {} ms; cached {} ms",
        png.width(),
        png.height(),
        video_time.as_millis(),
        cache_time.as_millis()
    );
    println!("PASS: native PDF PNG + system icons + missing-resource fallback");
    println!(
        "PASS: hung-provider termination {} ms; cancellation {} ms",
        timeout_time.as_millis(),
        start.elapsed().as_millis()
    );
    Ok(())
}
