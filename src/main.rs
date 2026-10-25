#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use anyhow::Context;

fn main() {
    rust_explorer::i18n::set_language(&rust_explorer::settings::Settings::load().language);
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_none_or(|arg| !arg.starts_with("--")) {
        // Old-version cleanup can touch many files; never delay the first frame.
        let _ = std::thread::spawn(rust_explorer::native_assets::cleanup_legacy_disk_cache);
    }
    let result = match args.get(1).map(String::as_str) {
        Some("--open-folder") => args
            .get(2)
            .context("缺少文件夹路径")
            .and_then(|path| rust_explorer::shell_bridge::open(std::path::Path::new(path))),
        Some("--folder-association-guard") => rust_explorer::default_browser::guard_main(
            args.get(2).map(String::as_str).unwrap_or(""),
        ),
        Some("--test-folder-browser") => rust_explorer::default_browser::begin_trial(
            args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120),
        ),
        Some("--confirm-folder-browser") => rust_explorer::default_browser::confirm_trial(),
        Some("--folder-association-status") => rust_explorer::default_browser::diagnostics()
            .and_then(|v| {
                println!("{}", serde_json::to_string_pretty(&v)?);
                Ok(())
            }),
        Some("--restore-folder-browser") => rust_explorer::default_browser::disable(),
        Some("--shell-parent") => (|| -> anyhow::Result<()> {
            let path: std::path::PathBuf =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::shell_folder::parent(&path)?)?
            );
            Ok(())
        })(),
        Some("--path-completions") => (|| -> anyhow::Result<()> {
            let text: String = serde_json::from_str(
                args.get(2)
                    .ok_or_else(|| anyhow::anyhow!("query missing"))?,
            )?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::path_completion::collect(&text)?)?
            );
            Ok(())
        })(),
        Some("--open-target") => (|| -> anyhow::Result<()> {
            let path: std::path::PathBuf =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::open_target::resolve_open_target(&path)?)?
            );
            Ok(())
        })(),
        Some("--quick-access") => rust_explorer::quick_access::snapshot().and_then(|items| {
            println!("{}", serde_json::to_string(&items)?);
            Ok(())
        }),
        Some("--file-hint") => (|| -> anyhow::Result<()> {
            let request: rust_explorer::file_hints::FileHintRequest = serde_json::from_str(
                args.get(2)
                    .ok_or_else(|| anyhow::anyhow!("request missing"))?,
            )?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::file_hints::collect(&request.path)?)?
            );
            Ok(())
        })(),
        Some("--thumbnail-server") => rust_explorer::native_assets::thumbnail_server(),
        Some("--shell-list") => (|| -> anyhow::Result<()> {
            let raw = args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?;
            let path: std::path::PathBuf =
                serde_json::from_str(raw).unwrap_or_else(|_| std::path::PathBuf::from(raw));
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::shell_folder::list(&path)?)?
            );
            Ok(())
        })(),
        Some("--shell-devices") => (|| -> anyhow::Result<()> {
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::shell_folder::device_paths()?)?
            );
            Ok(())
        })(),
        Some("--shell-resolve") => (|| -> anyhow::Result<()> {
            let text: String =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::shell_folder::resolve_display_path(&text)?)?
            );
            Ok(())
        })(),
        Some("--tree-metadata") => (|| -> anyhow::Result<()> {
            let request =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::volumes::children_with_attributes(request)?)?
            );
            Ok(())
        })(),
        Some("--volume-info") => (|| -> anyhow::Result<()> {
            let path: std::path::PathBuf =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::volumes::info(&path))?
            );
            Ok(())
        })(),
        Some("--tree-children") => (|| -> anyhow::Result<()> {
            let request =
                serde_json::from_str(args.get(2).ok_or_else(|| anyhow::anyhow!("path missing"))?)?;
            println!(
                "{}",
                serde_json::to_string(&rust_explorer::volumes::children(request)?)?
            );
            Ok(())
        })(),
        Some("--native-asset") => args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("缺少原生图像请求"))
            .and_then(|json| rust_explorer::native_assets::serve(json)),
        Some("--everything-query") => {
            rust_explorer::search::sdk_query(args.get(2).map(String::as_str).unwrap_or(""))
                .and_then(|v| {
                    println!("{}", serde_json::to_string(&v)?);
                    Ok(())
                })
        }
        Some("--rename-volume-admin") => args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("缺少驱动器重命名请求"))
            .and_then(|value| rust_explorer::volume_label::rename_admin(value)),
        // Elevated helper for the drive context menu's "Format" verb: the format
        // dialog has to run in an administrator process, and it is modal there.
        Some("--format-drive") => args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("缺少要格式化的驱动器"))
            .and_then(|value| rust_explorer::shell::format_drive(value)),
        Some("--shell-server") => {
            if let Some(pid) = args.get(2).and_then(|value| value.parse::<u32>().ok()) {
                rust_explorer::shell::watch_parent_process(pid);
            }
            rust_explorer::shell::serve_menu_requests()
        }
        Some("--shell-stdin") => rust_explorer::shell::read_request(std::io::stdin().lock())
            .and_then(rust_explorer::shell::dispatch),
        Some("--shell") => args
            .get(2)
            .ok_or_else(|| anyhow::anyhow!("缺少 Shell 请求"))
            .and_then(|v| serde_json::from_str(v).map_err(Into::into))
            .and_then(rust_explorer::shell::dispatch),
        _ => {
            #[cfg(feature = "desktop")]
            {
                let launch = match rust_explorer::launch_request::current() {
                    Ok(request) => request,
                    Err(error) => {
                        eprintln!("{error:#}");
                        // Release builds have no console: do not fail invisibly.
                        let message: Vec<u16> = format!("无法打开目标位置：\n{error:#}")
                            .encode_utf16()
                            .chain(Some(0))
                            .collect();
                        unsafe {
                            use windows::Win32::UI::WindowsAndMessaging::*;
                            MessageBoxW(
                                None,
                                windows::core::PCWSTR(message.as_ptr()),
                                windows::core::w!("Rust Explorer"),
                                MB_OK | MB_ICONERROR,
                            );
                        }
                        std::process::exit(1);
                    }
                };
                let (_instance, forwarded) = rust_explorer::instance::handoff(launch.as_ref());
                if !forwarded {
                    ui::run(launch);
                }
                Ok(())
            }
            #[cfg(not(feature = "desktop"))]
            {
                Err(anyhow::anyhow!("使用 --features desktop 构建图形界面"))
            }
        }
    };
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
#[cfg(feature = "desktop")]
mod input;
#[cfg(feature = "desktop")]
mod tab_tip;
mod ui;
#[cfg(feature = "desktop")]
mod visual;
