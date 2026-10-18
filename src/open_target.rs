//! Explicit-open target resolution. Run exclusively in the bounded helper.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OpenTarget {
    Directory(PathBuf),
    Font(PathBuf),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenDisposition {
    Current,
    Tab,
    Window,
}
pub fn folder_disposition(
    control: bool,
    shift: bool,
    shift_tab: bool,
    separate: bool,
) -> OpenDisposition {
    if control {
        OpenDisposition::Window
    } else if shift && shift_tab {
        OpenDisposition::Tab
    } else if separate {
        OpenDisposition::Window
    } else {
        OpenDisposition::Current
    }
}

pub fn is_font(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc" | "otc" | "fon" | "fnt"
        )
    })
}

/// Resolves at most eight shortcut hops, never runs IShellLink::Resolve (which
/// can search disks / network). The caller enforces the whole-process deadline.
/// Unsupported web/special Shell links retain their original Shell open verb.
pub fn resolve_open_target(path: &Path) -> Result<OpenTarget> {
    let original = std::path::absolute(path)?;
    let _runtime = if crate::preview::is_shortcut(&original) {
        Some(crate::native_preview::thumbnail_runtime()?)
    } else {
        None
    };
    let Some(target) = shortcut_chain(&original, crate::native_preview::shortcut_target)? else {
        return Ok(OpenTarget::File(original));
    };
    let metadata =
        std::fs::metadata(&target).with_context(|| format!("目标不可用：{}", target.display()))?;
    if metadata.is_dir() {
        return Ok(OpenTarget::Directory(canonical_directory(&target)?));
    }
    if is_font(&target) {
        return Ok(OpenTarget::Font(target));
    }
    Ok(OpenTarget::File(original))
}
fn shortcut_chain(
    original: &Path,
    mut resolve: impl FnMut(&Path) -> Result<PathBuf>,
) -> Result<Option<PathBuf>> {
    let mut target = original.to_path_buf();
    let mut visited = HashSet::new();
    for _ in 0..8 {
        ensure!(
            visited.insert(target.to_string_lossy().to_lowercase()),
            "快捷方式存在循环"
        );
        if crate::preview::is_shortcut(&target) {
            match resolve(&target) {
                Ok(next) => {
                    target = next;
                    continue;
                }
                // Non-file internet shortcuts / Shell namespace targets keep
                // Windows' original handling rather than being executed here.
                Err(_) => return Ok(None),
            }
        }
        return Ok(Some(target));
    }
    bail!("快捷方式跳转层数过多")
}
fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        Ok(PathBuf::from(format!(r"\\{unc}")))
    } else if let Some(disk) = text.strip_prefix(r"\\?\") {
        Ok(PathBuf::from(disk))
    } else {
        Ok(path)
    }
}

pub fn launch_font_preview(path: &Path) -> Result<()> {
    ensure!(is_font(path), "不是支持的字体文件");
    ensure!(std::fs::metadata(path)?.is_file(), "字体文件不存在");
    let mut directory = [0u16; 32768];
    let count = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut directory))
    } as usize;
    ensure!(
        count > 0 && count < directory.len(),
        "无法定位 Windows 字体预览器"
    );
    let viewer = PathBuf::from(String::from_utf16_lossy(&directory[..count])).join("fontview.exe");
    std::process::Command::new(viewer)
        .arg(std::path::absolute(path)?)
        .spawn()
        .context("无法打开 Windows 字体预览器")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_open_modifiers_have_explicit_precedence() {
        use OpenDisposition::*;
        assert_eq!(folder_disposition(false, false, true, false), Current);
        assert_eq!(folder_disposition(false, true, false, false), Current);
        assert_eq!(folder_disposition(false, true, true, false), Tab);
        assert_eq!(folder_disposition(false, true, true, true), Tab);
        assert_eq!(folder_disposition(true, true, true, true), Window);
        assert_eq!(folder_disposition(true, false, false, false), Window);
        assert_eq!(folder_disposition(false, false, false, true), Window);
    }
    #[test]
    fn font_extensions_are_explicit_and_case_insensitive() {
        for name in ["font.TTF", "font.otf", "font.ttc", "font.otc", "font.fon"] {
            assert!(is_font(Path::new(name)));
        }
        for name in ["font.ttf.exe", "font.woff2", "font.txt", "font"] {
            assert!(!is_font(Path::new(name)));
        }
    }
    #[test]
    fn explicit_open_classifies_owned_paths_without_executing() -> Result<()> {
        let folder = std::env::temp_dir().join(format!("RustExplorer-open-{}", std::process::id()));
        std::fs::create_dir_all(&folder)?;
        let file = folder.join("owned.txt");
        std::fs::write(&file, b"owned")?;
        let font = folder.join("owned.ttf");
        std::fs::write(&font, b"not installed or executed")?;
        assert_eq!(
            resolve_open_target(&folder)?,
            OpenTarget::Directory(canonical_directory(&folder)?)
        );
        assert_eq!(resolve_open_target(&file)?, OpenTarget::File(file.clone()));
        assert_eq!(resolve_open_target(&font)?, OpenTarget::Font(font.clone()));
        std::fs::remove_file(file)?;
        std::fs::remove_file(font)?;
        std::fs::remove_dir(folder)?;
        Ok(())
    }
    #[test]
    fn directory_shortcuts_and_chains_resolve_but_cycles_do_not_execute() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{
                System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile},
                UI::Shell::{IShellLinkW, ShellLink},
            },
            core::{Interface, PCWSTR},
        };
        let _runtime = crate::native_preview::thumbnail_runtime()?;
        let root =
            std::env::temp_dir().join(format!("RustExplorer-open-links-{}", std::process::id()));
        let folder = root.join("folder with spaces");
        std::fs::create_dir_all(&folder)?;
        let save = |target: &Path, path: &Path| -> Result<()> {
            let link: IShellLinkW =
                unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
            let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
            let output: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                link.SetPath(PCWSTR(target.as_ptr()))?;
            }
            let file: IPersistFile = link.cast()?;
            unsafe {
                file.Save(PCWSTR(output.as_ptr()), true)?;
            }
            Ok(())
        };
        let first = root.join("folder.lnk");
        let second = root.join("chain.lnk");
        save(&folder, &first).context("save directory link")?;
        save(&first, &second).context("save chained link")?;
        assert!(crate::fs::Entry::from_path(first.clone())?.link);
        let expected = OpenTarget::Directory(canonical_directory(&folder)?);
        assert_eq!(resolve_open_target(&first)?, expected);
        assert_eq!(resolve_open_target(&second)?, expected);
        let internet = root.join("folder.url");
        std::fs::write(
            &internet,
            format!(
                "[InternetShortcut]\r\nURL=file:///{}\r\n",
                folder
                    .to_string_lossy()
                    .replace('\\', "/")
                    .replace(' ', "%20")
            ),
        )?;
        assert_eq!(resolve_open_target(&internet)?, expected);
        for path in [first, second, internet] {
            std::fs::remove_file(path)?;
        }
        std::fs::remove_dir(folder)?;
        std::fs::remove_dir(root)?;
        Ok(())
    }
    #[test]
    fn malformed_shortcut_chains_have_loop_and_hop_guards() {
        assert!(
            shortcut_chain(Path::new("a.lnk"), |path| Ok(
                if path == Path::new("a.lnk") {
                    "b.lnk"
                } else {
                    "a.lnk"
                }
                .into()
            ))
            .is_err()
        );
        let mut hops = 0;
        assert!(
            shortcut_chain(Path::new("start.lnk"), |_| {
                hops += 1;
                Ok(format!("hop-{hops}.lnk").into())
            })
            .is_err()
        );
        assert_eq!(hops, 8);
    }
}
