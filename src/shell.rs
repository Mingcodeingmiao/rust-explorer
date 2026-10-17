use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    mem::size_of,
    path::{Path, PathBuf},
    ptr,
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, DataExchange::*, Memory::*, Ole::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{BOOL, Interface, PCSTR, PCWSTR, w},
};
#[path = "namespace_rename.rs"]
mod namespace_rename;
#[path = "native_menu.rs"]
mod native_menu;
#[path = "sharing_native.rs"]
mod sharing_native;

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value
        .as_ref()
        .encode_wide()
        .map(|v| if v == b'/' as u16 { b'\\' as u16 } else { v })
        .chain(Some(0))
        .collect()
}
struct Com;
impl Com {
    fn new() -> Result<Self> {
        unsafe {
            OleInitialize(None)?;
        }
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { OleUninitialize() }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum TransferMode {
    #[default]
    Auto,
    Copy,
    Move,
    Link,
}
#[derive(Serialize, Deserialize, Clone)]
pub enum Request {
    History {
        transaction: crate::file_history::Transaction,
    },
    RecordCreated {
        path: PathBuf,
    },
    QuickAccessPin {
        path: PathBuf,
        pinned: bool,
    },
    VolumeLabel {
        path: PathBuf,
    },
    RenameVolume {
        path: PathBuf,
        label: String,
    },
    FontPreview {
        path: PathBuf,
    },
    SystemProperties,
    Transfer {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        copy_only: bool,
        #[serde(default)]
        mode: TransferMode,
    },
    Share {
        paths: Vec<PathBuf>,
    },
    Shortcut {
        target: PathBuf,
        destination: PathBuf,
    },
    Open {
        path: PathBuf,
    },
    Properties {
        path: PathBuf,
    },
    PropertiesMany {
        paths: Vec<PathBuf>,
    },
    Menu {
        paths: Vec<PathBuf>,
        folder: PathBuf,
        x: i32,
        y: i32,
    },
    /// Real Shell item/drive menu; app-owned background view commands remain.
    ThemedMenu {
        paths: Vec<PathBuf>,
        folder: PathBuf,
        x: i32,
        y: i32,
        dark: bool,
        view: u8,
        sort: String,
        descending: bool,
        group: String,
        #[serde(default)]
        undo: bool,
        #[serde(default)]
        redo: bool,
    },
    SharingAccess {
        path: PathBuf,
        #[serde(default)]
        remove: bool,
    },
    InspectSharing {
        path: PathBuf,
    },
    ComputerMenu {
        x: i32,
        y: i32,
        dark: bool,
        expanded: bool,
    },
    AddNetworkLocation,
    RenameComputer {
        dark: bool,
    },
    ComputerName,
    /// Read-only inventory; never display or invoke a Shell command.
    InspectMenu {
        paths: Vec<PathBuf>,
        folder: PathBuf,
        #[serde(default)]
        computer: bool,
        #[serde(default)]
        host_commands: bool,
    },
    Copy {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        moving: bool,
    },
    Delete {
        paths: Vec<PathBuf>,
    },
    /// Separate from recycle deletion: callers must display an explicit
    /// irreversible-operation confirmation before dispatching this request.
    PermanentDelete {
        paths: Vec<PathBuf>,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
}
pub fn execute(request: Request) -> Result<()> {
    execute_output(request).map(|_| ())
}
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub fn read_request(input: impl std::io::Read) -> Result<Request> {
    use std::io::Read;
    let mut bytes = Vec::new();
    input
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_REQUEST_BYTES,
        "系统操作请求超过 32 MiB，未执行操作"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
fn read_bounded(mut input: impl std::io::Read, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    (&mut input)
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        // Drain instead of abandoning a full pipe while the parent waits: a
        // helper must never block indefinitely trying to finish its response.
        std::io::copy(&mut input, &mut std::io::sink())?;
        bail!("系统操作响应超过内存限制");
    }
    Ok(bytes)
}
pub fn execute_output(request: Request) -> Result<String> {
    use std::{io::Write, process::Stdio};
    let pin_deadline = matches!(&request, Request::QuickAccessPin { .. });
    let payload = serde_json::to_vec(&request)?;
    anyhow::ensure!(
        payload.len() <= MAX_REQUEST_BYTES,
        "系统操作请求超过 32 MiB，未执行操作"
    );
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("--shell-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
    let mut child = cmd.spawn()?;
    // Native menus are displayed by this child, not by the GPUI parent window.
    // Forward the current application's foreground permission to that helper.
    unsafe {
        let _ = AllowSetForegroundWindow(child.id());
    }
    let mut input = child.stdin.take().context("helper stdin")?;
    let stdout = child.stdout.take().context("helper stdout")?;
    let stderr = child.stderr.take().context("helper stderr")?;
    let output_reader = std::thread::spawn(move || read_bounded(stdout, 96 * 1024 * 1024));
    let error_reader = std::thread::spawn(move || read_bounded(stderr, 1024 * 1024));
    let writer = std::thread::spawn(move || input.write_all(&payload));
    let mut timed_out = false;
    let status = if pin_deadline {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() >= std::time::Duration::from_secs(5) {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    } else {
        child.wait()?
    };
    let written = writer
        .join()
        .map_err(|_| anyhow::anyhow!("helper stdin writer failed"))?;
    let stdout = output_reader
        .join()
        .map_err(|_| anyhow::anyhow!("helper stdout reader failed"))??;
    let stderr = error_reader
        .join()
        .map_err(|_| anyhow::anyhow!("helper stderr reader failed"))??;
    anyhow::ensure!(
        !timed_out,
        "系统快速访问操作超过 5 秒，请刷新后确认当前固定状态"
    );
    if !status.success() {
        let error = String::from_utf8_lossy(&stderr);
        if let Some(response) = crate::file_history::with_error(
            &String::from_utf8_lossy(&stdout),
            error.trim().to_string(),
        ) {
            return Ok(response);
        }
        bail!(
            "{}",
            if error.trim().is_empty() {
                format!("系统操作进程已结束（{}）", status)
            } else {
                error.into_owned()
            }
        );
    }
    written.context("写入系统操作请求失败")?;
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}
#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn stdin_accepts_requests_larger_than_windows_command_lines() {
        let request = Request::Copy {
            paths: (0..1000)
                .map(|index| {
                    PathBuf::from(format!("C:\\owned-fixture\\{index}-{}", "x".repeat(100)))
                })
                .collect(),
            destination: PathBuf::from("C:\\owned-fixture-target"),
            moving: false,
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        assert!(bytes.len() > 32768);
        assert!(
            matches!(read_request(bytes.as_slice()).unwrap(),Request::Copy {paths,..} if paths.len()==1000)
        );
    }
    #[test]
    fn oversized_input_is_rejected_and_oversized_output_is_drained() {
        use std::io::Read;
        let error = read_request(std::io::repeat(b' ').take(MAX_REQUEST_BYTES as u64 + 1))
            .err()
            .unwrap();
        assert!(error.to_string().contains("32 MiB"));
        let mut reader = std::io::Cursor::new(vec![b'x'; 100]);
        assert!(read_bounded(&mut reader, 8).is_err());
        assert_eq!(reader.position(), 100);
    }
}
pub fn dispatch(request: Request) -> Result<()> {
    let _com = Com::new()?;
    match request {
        Request::History { transaction } => crate::file_history::apply(transaction),
        Request::RecordCreated { path } => crate::file_history::record_created(&path),
        Request::QuickAccessPin { path, pinned } => crate::quick_access::set_pinned(&path, pinned),
        Request::VolumeLabel { path } => {
            println!(
                "{}",
                serde_json::to_string(&crate::volume_label::read(&path)?)?
            );
            Ok(())
        }
        Request::RenameVolume { path, label } => crate::volume_label::rename(&path, &label),
        Request::FontPreview { path } => crate::open_target::launch_font_preview(&path),
        Request::SystemProperties => {
            let system = std::env::var_os("WINDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
            std::process::Command::new(system.join("System32/control.exe"))
                .args(["/name", "Microsoft.System"])
                .spawn()?;
            Ok(())
        }
        Request::Transfer {
            paths,
            destination,
            copy_only,
            mode,
        } => unsafe {
            anyhow::ensure!(destination.is_dir(), "目标文件夹不存在");
            let target = std::fs::canonicalize(&destination)?;
            let target_key = target.to_string_lossy().to_lowercase();
            // Resolve junctions before rejecting a drop onto itself or a descendant.
            for path in &paths {
                let source = std::fs::canonicalize(path)?;
                let key = source.to_string_lossy().to_lowercase();
                anyhow::ensure!(
                    target_key != key && !target_key.starts_with(&(key + "\\")),
                    "不能将项目放入自身或其子文件夹"
                );
            }
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = crate::file_history::Recorder::attach(&op, &paths)?;
            op.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0))?;
            let dst: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(wide(&destination).as_ptr()), None)?;
            let target_volume = volume_identity(&target);
            for path in paths {
                if mode == TransferMode::Link {
                    let stem = path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy();
                    let mut link_path = destination.join(format!("{stem} - 快捷方式.lnk"));
                    let mut suffix = 2;
                    while link_path.exists() {
                        link_path = destination.join(format!("{stem} - 快捷方式 ({suffix}).lnk"));
                        suffix += 1;
                    }
                    let link: IShellLinkW =
                        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    link.SetPath(PCWSTR(wide(&path).as_ptr()))?;
                    if let Some(parent) = path.parent() {
                        link.SetWorkingDirectory(PCWSTR(wide(parent).as_ptr()))?;
                    }
                    let persist: IPersistFile = link.cast()?;
                    persist.Save(PCWSTR(wide(&link_path).as_ptr()), false)?;
                    _history.created(&link_path);
                    continue;
                }
                let src: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)?;
                let moving = mode == TransferMode::Move
                    || (mode == TransferMode::Auto
                        && !copy_only
                        && target_volume.is_some()
                        && volume_identity(&std::fs::canonicalize(&path)?) == target_volume);
                if moving {
                    op.MoveItem(&src, &dst, None, None)?;
                } else {
                    op.CopyItem(&src, &dst, None, None)?;
                }
            }
            // Shortcut persistence has already completed; IFileOperation rejects
            // PerformOperations with an empty queue on some Windows builds.
            if mode == TransferMode::Link {
                return Ok(());
            }
            op.PerformOperations()?;
            anyhow::ensure!(!op.GetAnyOperationsAborted()?.as_bool(), "操作已取消");
            Ok(())
        },
        Request::Share { paths } => {
            anyhow::ensure!(paths.len() == 1, "请选择一个文件，或先压缩为 ZIP 后共享");
            let path = wide(&paths[0]);
            let mut info = SHELLEXECUTEINFOW {
                cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC,
                lpVerb: w!("Windows.ModernShare"),
                lpFile: PCWSTR(path.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            unsafe { ShellExecuteExW(&mut info) }.context("当前系统无法打开原生共享界面")?;
            unsafe {
                pump_property_windows();
            }
            Ok(())
        }
        Request::Shortcut {
            target,
            destination,
        } => unsafe {
            if !target.exists() {
                bail!("快捷方式目标不存在");
            }
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(wide(&target).as_ptr()))?;
            if let Some(parent) = target.parent() {
                link.SetWorkingDirectory(PCWSTR(wide(parent).as_ptr()))?;
            }
            let persist: IPersistFile = link.cast()?;
            anyhow::ensure!(!destination.exists(), "Shortcut destination already exists");
            persist.Save(PCWSTR(wide(&destination).as_ptr()), true)?;
            crate::file_history::record_created(&destination)?;
            Ok(())
        },
        Request::Open { path } => open(&path),
        Request::Properties { path } => properties(&path),
        Request::PropertiesMany { paths } => properties_many(&paths),
        Request::Menu {
            paths,
            folder,
            x,
            y,
        } => context_menu(&paths, &folder, x, y),
        Request::ThemedMenu {
            paths,
            folder,
            x,
            y,
            dark,
            view,
            sort,
            descending,
            group,
            undo,
            redo,
        } => context_menu_options(
            &paths,
            &folder,
            x,
            y,
            Some(dark),
            false,
            false,
            false,
            Some(native_menu::Options {
                view,
                sort,
                descending,
                group,
                undo,
                redo,
            }),
        ),
        Request::ComputerMenu {
            x,
            y,
            dark,
            expanded,
        } => context_menu_options(
            &[],
            Path::new("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
            x,
            y,
            Some(dark),
            true,
            expanded,
            false,
            None,
        ),
        Request::AddNetworkLocation => add_network_location(),
        Request::RenameComputer { dark } => namespace_rename::computer(dark),
        Request::ComputerName => {
            println!("computer-name:{}", namespace_rename::computer_name()?);
            Ok(())
        }
        Request::InspectMenu {
            paths,
            folder,
            computer,
            host_commands,
        } => context_menu_options(
            &paths,
            &folder,
            0,
            0,
            None,
            computer,
            false,
            true,
            host_commands.then(|| native_menu::Options {
                view: 5,
                sort: "name".into(),
                descending: false,
                group: String::new(),
                undo: true,
                redo: true,
            }),
        ),
        Request::SharingAccess { path, remove } => sharing_access(&path, remove),
        Request::InspectSharing { path } => {
            println!(
                "{}",
                serde_json::to_string(&sharing_native::inspect(&path)?)?
            );
            Ok(())
        }
        Request::Copy {
            paths,
            destination,
            moving,
        } => unsafe {
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .context("创建文件操作")?;
            let _history = crate::file_history::Recorder::attach(&op, &paths)?;
            op.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0))
                .context("设置文件操作标志")?;
            let dst: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(wide(&destination).as_ptr()), None)
                    .context("解析目标文件夹")?;
            for path in paths {
                let src: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)
                        .context("解析源文件")?;
                if moving {
                    op.MoveItem(&src, &dst, None, None).context("安排移动")?;
                } else {
                    op.CopyItem(&src, &dst, None, None).context("安排复制")?;
                }
            }
            op.PerformOperations().context("执行文件操作")?;
            if op.GetAnyOperationsAborted()?.as_bool() {
                bail!("操作已取消");
            }
            Ok(())
        },
        Request::PermanentDelete { paths } => unsafe {
            crate::file_history::barrier("Permanent deletion cannot be undone");
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            // Deliberately omit ALLOWUNDO, ADDUNDORECORD and RECYCLEONDELETE.
            // Never retry this operation through Request::Delete on failure.
            op.SetOperationFlags(FILEOPERATION_FLAGS(
                FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0,
            ))?;
            for path in paths {
                anyhow::ensure!(
                    path.is_absolute() && path.parent().is_some(),
                    "Refuse permanent deletion of a volume root"
                );
                let item: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)?;
                op.DeleteItem(&item, None)?;
            }
            op.PerformOperations()?;
            if op.GetAnyOperationsAborted()?.as_bool() {
                bail!("永久删除已取消");
            }
            Ok(())
        },
        Request::Delete { paths } => unsafe {
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = crate::file_history::Recorder::attach(&op, &paths)?;
            op.SetOperationFlags(FILEOPERATION_FLAGS(
                FOF_ALLOWUNDO.0
                    | FOFX_ADDUNDORECORD.0
                    | FOFX_RECYCLEONDELETE.0
                    | FOF_WANTNUKEWARNING.0,
            ))?;
            for path in paths {
                let item: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)?;
                op.DeleteItem(&item, None)?;
            }
            op.PerformOperations()?;
            if op.GetAnyOperationsAborted()?.as_bool() {
                bail!("操作已取消");
            }
            Ok(())
        },
        Request::Rename { path, name } => unsafe {
            crate::fs::valid_name(&name)?;
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = crate::file_history::Recorder::attach(&op, std::slice::from_ref(&path))?;
            op.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0))?;
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide(&path).as_ptr()), None)?;
            op.RenameItem(&item, PCWSTR(wide(&name).as_ptr()), None)?;
            op.PerformOperations()?;
            if op.GetAnyOperationsAborted()?.as_bool() {
                bail!("操作已取消");
            }
            Ok(())
        },
    }
}

fn volume_identity(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    let mut root = [0u16; 32768];
    let mut guid = [0u16; 128];
    unsafe {
        GetVolumePathNameW(PCWSTR(wide(path).as_ptr()), &mut root).ok()?;
        if GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), &mut guid).is_ok() {
            return Some(
                String::from_utf16_lossy(&guid[..guid.iter().position(|v| *v == 0)?])
                    .to_lowercase(),
            );
        }
    }
    // A server share has no local volume GUID. Unknown identity safely means copy.
    None
}

pub fn open(path: &Path) -> Result<()> {
    if crate::open_target::is_font(path) {
        return crate::open_target::launch_font_preview(path);
    }
    if path.is_dir() {
        let system = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        std::process::Command::new(system.join("explorer.exe"))
            .arg(path)
            .spawn()
            .context("打开系统资源管理器")?;
        return Ok(());
    }
    let path_wide = wide(path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(path_wide.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
        .with_context(|| format!("系统无法打开 {}", path.display()))?;
    Ok(())
}
pub fn properties(path: &Path) -> Result<()> {
    unsafe {
        let path = wide(path);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC,
            lpVerb: w!("properties"),
            lpFile: PCWSTR(path.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        ShellExecuteExW(&mut info)?;
        pump_property_windows();
    }
    Ok(())
}

pub fn properties_many(paths: &[PathBuf]) -> Result<()> {
    if paths.len() == 1 {
        return properties(&paths[0]);
    }
    unsafe {
        let pidls: Vec<_> = paths
            .iter()
            .map(|p| Pidl(ILCreateFromPathW(PCWSTR(wide(p).as_ptr()))))
            .collect();
        if pidls.iter().any(|p| p.0.is_null()) {
            bail!("无法解析属性项目");
        }
        let pointers: Vec<_> = pidls
            .iter()
            .map(|p| p.0 as *const Common::ITEMIDLIST)
            .collect();
        let items = SHCreateShellItemArrayFromIDLists(&pointers)?;
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
        SHMultiFileProperties(&data, 0)?;
        pump_property_windows();
        Ok(())
    }
}
// Property sheets are modeless and can be created after ShellExecuteEx returns.
// Keep the isolated STA alive and dispatch messages until its sheets close.
unsafe fn pump_property_windows() {
    unsafe extern "system" fn count(hwnd: HWND, param: LPARAM) -> BOOL {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        let mut bounds = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut bounds) };
        if pid == std::process::id()
            && bounds.right - bounds.left > 8
            && bounds.bottom - bounds.top > 8
            && unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            unsafe {
                *(param.0 as *mut usize) += 1;
            }
        }
        BOOL(1)
    }
    let start = std::time::Instant::now();
    let mut observed = false;
    loop {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let mut windows = 0usize;
        // Thread id zero is not accepted by EnumThreadWindows.
        let _ = unsafe { EnumWindows(Some(count), LPARAM(&mut windows as *mut _ as isize)) };
        observed |= windows > 0;
        if windows == 0 && (observed || start.elapsed() > std::time::Duration::from_secs(3)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}
pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
pub fn drives() -> Vec<PathBuf> {
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char)))
        .collect()
}
pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        CoTaskMemFree(Some(p.0.cast()));
        Some(path)
    }
}
pub fn places() -> Vec<(String, PathBuf)> {
    [
        ("桌面", FOLDERID_Desktop),
        ("下载", FOLDERID_Downloads),
        ("文档", FOLDERID_Documents),
        ("图片", FOLDERID_Pictures),
        ("音乐", FOLDERID_Music),
        ("视频", FOLDERID_Videos),
    ]
    .into_iter()
    .filter_map(|(n, id)| known_folder(&id).map(|p| (n.into(), p)))
    .collect()
}

struct Clipboard;
impl Clipboard {
    fn open() -> Result<Self> {
        unsafe {
            OpenClipboard(None)?;
        }
        Ok(Self)
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
unsafe fn put_data(format: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let ptr = GlobalLock(memory);
        if ptr.is_null() {
            let _ = GlobalFree(Some(memory));
            bail!("剪贴板分配失败");
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        GlobalUnlock(memory).ok();
        if let Err(e) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(e.into());
        }
        Ok(()) // Windows owns memory after SetClipboardData succeeds.
    }
}
pub fn copy_to_clipboard(paths: &[PathBuf], cut: bool) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let _clipboard = Clipboard::open()?;
    unsafe {
        EmptyClipboard()?;
        let names: Vec<u16> = paths.iter().flat_map(wide).chain(Some(0)).collect();
        let mut bytes = Vec::with_capacity(20 + names.len() * 2);
        bytes.extend_from_slice(&20u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        for v in names {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        put_data(15, &bytes)?;
        let effect = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        put_data(effect, &(if cut { 2u32 } else { 1u32 }).to_le_bytes())?;
    }
    Ok(())
}
pub fn paste_from_clipboard() -> Result<(Vec<PathBuf>, bool)> {
    let _clipboard = Clipboard::open()?;
    unsafe {
        let data = GetClipboardData(15).context("剪贴板中没有文件")?;
        let drop = HDROP(data.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None);
            let mut text = vec![0u16; len as usize + 1];
            DragQueryFileW(drop, i, Some(&mut text));
            paths.push(PathBuf::from(String::from_utf16_lossy(
                &text[..len as usize],
            )));
        }
        let format = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        let mut cut = false;
        if let Ok(effect) = GetClipboardData(format) {
            let memory = HGLOBAL(effect.0);
            let ptr = GlobalLock(memory);
            if !ptr.is_null() {
                if GlobalSize(memory) >= 4 {
                    cut = ptr.cast::<u32>().read_unaligned() & 2 != 0;
                }
                GlobalUnlock(memory).ok();
            }
        }
        Ok((paths, cut))
    }
}
pub fn clear_clipboard() {
    if let Ok(_clip) = Clipboard::open() {
        unsafe {
            let _ = EmptyClipboard();
        }
    }
}

thread_local! {static MENU:RefCell<Option<IContextMenu>>=const{RefCell::new(None)};}
unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_CANCELMODE || (msg == WM_ACTIVATEAPP && w.0 == 0) {
        unsafe {
            let _ = EndMenu();
        }
        return LRESULT(0);
    }
    if [WM_INITMENUPOPUP, WM_DRAWITEM, WM_MEASUREITEM, WM_MENUCHAR].contains(&msg) {
        let handled = MENU.with(|menu| {
            let borrow = menu.borrow();
            let menu = borrow.as_ref()?;
            if let Ok(menu) = menu.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if unsafe { menu.HandleMenuMsg2(msg, w, l, Some(&mut result)) }.is_ok() {
                    return Some(result);
                }
            }
            if msg != WM_MENUCHAR {
                if let Ok(menu) = menu.cast::<IContextMenu2>() {
                    if unsafe { menu.HandleMenuMsg(msg, w, l) }.is_ok() {
                        return Some(LRESULT(0));
                    }
                }
            }
            None
        });
        if let Some(result) = handled {
            return result;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}
struct Pidl(*mut Common::ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
struct MenuWindow(HWND);
impl Drop for MenuWindow {
    fn drop(&mut self) {
        MENU.with(|v| *v.borrow_mut() = None);
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
struct Popup(HMENU);
impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

fn add_network_location() -> Result<()> {
    let mut directory = [0u16; 32768];
    let length = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut directory))
    } as usize;
    anyhow::ensure!(
        length > 0 && length < directory.len(),
        "Windows System32 directory is unavailable"
    );
    let system = PathBuf::from(String::from_utf16_lossy(&directory[..length]));
    use std::os::windows::process::CommandExt;
    // Matches the installed Microsoft CommandStore Windows.AddNetworkLocation.
    // This only opens the wizard; no location is added without its own UI.
    std::process::Command::new(system.join("rundll32.exe"))
        .arg(format!(
            "{},AddNetPlaceRunDll",
            system.join("shwebsvc.dll").display()
        ))
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}

fn sharing_access(path: &Path, remove: bool) -> Result<()> {
    anyhow::ensure!(
        path.is_absolute() && path.is_dir(),
        "请选择要设置共享的文件夹"
    );
    // The two installed CommandStore handlers are distinct native workflows.
    // Never implement removal by changing ACLs, and never silently turn it
    // into the Specific Users workflow when the handler is unavailable.
    match invoke_sharing_menu(path, remove) {
        Ok(()) => return Ok(()),
        Err(error) if remove => return Err(error.context("Windows 无法打开删除访问权限界面")),
        Err(_) => {}
    }
    unsafe {
        use windows::Win32::System::LibraryLoader::*;
        // Official, verified two-argument entry point. It opens Windows UI;
        // no permission, share or ACL is changed by this application.
        if let Ok(module) = LoadLibraryExW(w!("ntshrui.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
            let entry = GetProcAddress(module, windows::core::s!("ShowShareFolderUI"));
            if let Some(entry) = entry {
                let show: unsafe extern "system" fn(HWND, PCWSTR) -> windows::core::HRESULT =
                    std::mem::transmute(entry);
                let result = show(HWND::default(), PCWSTR(wide(path).as_ptr()));
                let _ = FreeLibrary(module);
                result.ok()?;
                return Ok(());
            }
            let _ = FreeLibrary(module);
        }
        if SHObjectProperties(
            None,
            SHOP_FILEPATH,
            PCWSTR(wide(path).as_ptr()),
            w!("Sharing"),
        )
        .as_bool()
        {
            Ok(())
        } else {
            bail!("Windows 无法打开此文件夹的共享设置")
        }
    }
}

fn normalized_menu_title(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '&' | '.' | '…') && !c.is_whitespace())
        .collect()
}
fn invoke_sharing_menu(path: &Path, remove: bool) -> Result<()> {
    unsafe {
        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let class = w!("RustExplorer.SharingCommand");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(menu_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: class,
            ..Default::default()
        });
        let owner = MenuWindow(CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class,
            w!("Rust Explorer"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?);
        SetLayeredWindowAttributes(owner.0, COLORREF(0), 0, LWA_ALPHA)?;
        let pidl = Pidl(ILCreateFromPathW(PCWSTR(wide(path).as_ptr())));
        anyhow::ensure!(!pidl.0.is_null(), "Windows 无法解析此文件夹");
        let parent: IShellFolder = SHBindToParent(pidl.0, None)?;
        let children = [ILFindLastID(pidl.0) as *const Common::ITEMIDLIST];
        let menu: IContextMenu = parent.GetUIObjectOf(owner.0, &children, None)?;
        let popup = Popup(CreatePopupMenu()?);
        menu.QueryContextMenu(
            popup.0,
            0,
            1,
            0x6fff,
            CMF_NORMAL | CMF_CANRENAME | CMF_EXPLORE,
        )
        .ok()?;
        MENU.with(|value| *value.borrow_mut() = Some(menu.clone()));
        // GetTitle is a read-only, localized identity query. The private command
        // requires Explorer site state for GetState/Invoke, so invoke the same
        // command through the fully initialized Shell context menu instead.
        let title = normalized_menu_title(&sharing_native::title(path, remove)?);
        let entries = menu_entries(&menu, popup.0, 0);
        let target = entries
            .iter()
            .find(|entry| entry.verb == "windows.share")
            .and_then(|entry| {
                entry
                    .children
                    .iter()
                    .find(|child| normalized_menu_title(&child.label) == title)
            });
        let target = target.context("系统共享菜单没有提供对应的访问权限操作")?;
        anyhow::ensure!(
            target.enabled && target.id > 0 && target.id <= 0x6fff,
            "当前文件夹不支持此共享操作"
        );
        let _ = ShowWindow(owner.0, SW_SHOW);
        let _ = SetForegroundWindow(owner.0);
        let mut info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: SEE_MASK_UNICODE,
            hwnd: owner.0,
            lpVerb: PCSTR((target.id as usize - 1) as *const u8),
            lpVerbW: PCWSTR((target.id as usize - 1) as *const u16),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        menu.InvokeCommand(&mut info as *mut _ as *const CMINVOKECOMMANDINFO)?;
        pump_property_windows();
        Ok(())
    }
}

fn context_menu(paths: &[PathBuf], folder: &Path, x: i32, y: i32) -> Result<()> {
    context_menu_options(
        paths,
        folder,
        x,
        y,
        Some(crate::theme::Theme::system().dark),
        false,
        false,
        false,
        None,
    )
}

#[derive(Serialize)]
struct MenuEntry {
    label: String,
    verb: String,
    id: u32,
    enabled: bool,
    children: Vec<MenuEntry>,
}
unsafe fn menu_entries(menu: &IContextMenu, popup: HMENU, depth: u8) -> Vec<MenuEntry> {
    if depth > 5 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for index in 0..unsafe { GetMenuItemCount(Some(popup)) }.clamp(0, 256) {
        let mut text = [0u16; 1024];
        let mut item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STRING | MIIM_ID | MIIM_SUBMENU | MIIM_STATE | MIIM_FTYPE,
            dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
            cch: text.len() as u32,
            ..Default::default()
        };
        if unsafe { GetMenuItemInfoW(popup, index as u32, true, &mut item) }.is_err()
            || item.fType.contains(MFT_SEPARATOR)
        {
            continue;
        }
        let children = if !item.hSubMenu.is_invalid() {
            if let Ok(extension) = menu.cast::<IContextMenu3>() {
                let _ = unsafe {
                    extension.HandleMenuMsg2(
                        WM_INITMENUPOPUP,
                        WPARAM(item.hSubMenu.0 as usize),
                        LPARAM(index as isize),
                        None,
                    )
                };
            } else if let Ok(extension) = menu.cast::<IContextMenu2>() {
                let _ = unsafe {
                    extension.HandleMenuMsg(
                        WM_INITMENUPOPUP,
                        WPARAM(item.hSubMenu.0 as usize),
                        LPARAM(index as isize),
                    )
                };
            }
            unsafe { menu_entries(menu, item.hSubMenu, depth + 1) }
        } else {
            Vec::new()
        };
        result.push(MenuEntry {
            label: String::from_utf16_lossy(
                &text[..text
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(text.len())],
            ),
            verb: canonical_verb(menu, item.wID),
            id: item.wID,
            enabled: !item.fState.contains(MFS_DISABLED),
            children,
        });
    }
    result
}
fn canonical_verb(menu: &IContextMenu, command: u32) -> String {
    if command == 0 || command > 0x6fff {
        return String::new();
    }
    let mut verb = [0u8; 256];
    unsafe {
        let _ = menu.GetCommandString(
            command as usize - 1,
            GCS_VERBA,
            None,
            windows::core::PSTR(verb.as_mut_ptr()),
            verb.len() as u32,
        );
    }
    String::from_utf8_lossy(
        &verb[..verb
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(verb.len())],
    )
    .to_ascii_lowercase()
}
#[allow(clippy::too_many_arguments)]
fn context_menu_options(
    paths: &[PathBuf],
    folder: &Path,
    x: i32,
    y: i32,
    dark: Option<bool>,
    computer: bool,
    expanded: bool,
    inspect: bool,
    options: Option<native_menu::Options>,
) -> Result<()> {
    unsafe {
        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let class = w!("RustExplorer.ShellMenu");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(menu_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class,
            w!("Rust Explorer"),
            WS_POPUP | if inspect { WINDOW_STYLE(0) } else { WS_VISIBLE },
            x,
            y,
            1,
            1,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        let _owner = MenuWindow(hwnd);
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA)?;
        if let Some(dark) = dark {
            native_menu::configure_theme(hwnd, dark);
        }
        let menu: IContextMenu = if computer {
            let mut raw = ptr::null_mut();
            SHParseDisplayName(
                w!("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
                None,
                &mut raw,
                0,
                None,
            )?;
            let pidl = Pidl(raw);
            let parent: IShellFolder = SHBindToParent(pidl.0, None)?;
            let children = [ILFindLastID(pidl.0) as *const Common::ITEMIDLIST];
            parent.GetUIObjectOf(hwnd, &children, None)?
        } else if paths.is_empty() {
            let pidl = Pidl(ILCreateFromPathW(PCWSTR(wide(folder).as_ptr())));
            if pidl.0.is_null() {
                bail!("无法读取文件夹菜单");
            }
            let desktop: IShellFolder = SHGetDesktopFolder()?;
            let folder: IShellFolder = desktop.BindToObject(pidl.0, None)?;
            folder.CreateViewObject(hwnd)?
        } else {
            let mut items = Vec::new();
            for path in paths {
                let pidl = Pidl(ILCreateFromPathW(PCWSTR(wide(path).as_ptr())));
                if pidl.0.is_null() {
                    bail!("无法读取文件菜单");
                }
                items.push(pidl);
            }
            let parent: IShellFolder = SHBindToParent(items[0].0, None)?;
            // Cross-folder selections are exposed through IShellItemArray's data object.
            let same_parent = paths.iter().all(|p| p.parent() == paths[0].parent());
            if !same_parent {
                bail!("跨目录多选菜单请先打开所在位置；复制/移动/删除仍支持跨目录多选");
            }
            let children: Vec<*const Common::ITEMIDLIST> = items
                .iter()
                .map(|v| ILFindLastID(v.0) as *const _)
                .collect();
            parent.GetUIObjectOf(hwnd, &children, None)?
        };
        let popup = Popup(CreatePopupMenu()?);
        menu.QueryContextMenu(
            popup.0,
            0,
            1,
            0x6fff,
            CMF_NORMAL | CMF_CANRENAME | CMF_EXPLORE,
        )
        .ok()?;
        MENU.with(|v| *v.borrow_mut() = Some(menu.clone()));
        let commands = if let Some(options) = &options {
            // Shell view commands need our own IShellView state; keep all other
            // provider commands, including the real New and sharing submenus.
            for position in (0..GetMenuItemCount(Some(popup.0))).rev() {
                let verb = canonical_verb(&menu, GetMenuItemID(popup.0, position));
                if matches!(verb.as_str(), "undo" | "redo")
                    || (paths.is_empty()
                        && matches!(
                            verb.as_str(),
                            "view" | "arrange" | "sort" | "groupby" | "refresh"
                        ))
                {
                    DeleteMenu(popup.0, position as u32, MF_BYPOSITION)?;
                }
            }
            native_menu::augment(popup.0, options, paths.is_empty())?
        } else {
            Vec::new()
        };
        if inspect {
            println!(
                "{}",
                serde_json::to_string(&menu_entries(&menu, popup.0, 0))?
            );
            return Ok(());
        }
        if computer {
            let native_quick_pin = (0..GetMenuItemCount(Some(popup.0)))
                .any(|index| canonical_verb(&menu, GetMenuItemID(popup.0, index)) == "pintohome");
            for (position, id, label) in [
                (0, 0x8000, if expanded { "折叠" } else { "展开" }),
                (1, 0x8001, "在新标签页中打开"),
                (2, 0x8002, "在新窗口中打开"),
            ] {
                InsertMenuW(
                    popup.0,
                    position,
                    MF_BYPOSITION | MF_STRING,
                    id,
                    PCWSTR(wide(label).as_ptr()),
                )?;
            }
            if !native_quick_pin {
                InsertMenuW(
                    popup.0,
                    3,
                    MF_BYPOSITION | MF_STRING,
                    0x8003,
                    w!("固定到快速访问"),
                )?;
            }
            InsertMenuW(
                popup.0,
                if native_quick_pin { 3 } else { 4 },
                MF_BYPOSITION | MF_SEPARATOR,
                0,
                PCWSTR::null(),
            )?;
            AppendMenuW(popup.0, MF_STRING, 0x8004, w!("添加网络位置(&L)…"))?;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow(hwnd);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
        let command = TrackPopupMenuEx(
            popup.0,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            x,
            y,
            hwnd,
            None,
        )
        .0;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        if let Some(action) = (command as usize)
            .checked_sub(0x9000)
            .and_then(|index| commands.get(index))
        {
            println!("command:{action}");
            return Ok(());
        }
        if computer {
            if command == 0x8004 {
                return add_network_location();
            }
            let action = match command {
                0x8000 => Some("computer-expand"),
                0x8001 => Some("computer-tab"),
                0x8002 => Some("computer-window"),
                0x8003 => Some("computer-pin"),
                _ => None,
            };
            if let Some(action) = action {
                println!("command:{action}");
                return Ok(());
            }
        }
        if command > 0 {
            let verb = canonical_verb(&menu, command as u32);
            if computer && verb == "pintohome" {
                println!("command:computer-pin");
                return Ok(());
            }
            if computer && verb == "rename" {
                return namespace_rename::computer(dark.unwrap_or(false));
            }
            if computer && matches!(verb.as_str(), "properties" | "rename" | "open" | "explore") {
                println!(
                    "command:{}",
                    match verb.as_str() {
                        "properties" => "computer-properties",
                        "rename" => "computer-rename",
                        _ => "computer-open",
                    }
                );
                return Ok(());
            }
            if verb == "rename" {
                println!("rename");
                return Ok(());
            }
            if verb == "properties" {
                println!("properties");
                return Ok(());
            }
            if matches!(verb.as_str(), "open" | "explore") && !paths.is_empty() {
                println!("command:file-open");
                return Ok(());
            }
            let mut info = CMINVOKECOMMANDINFOEX {
                cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: SEE_MASK_UNICODE,
                hwnd,
                lpVerb: PCSTR((command as usize - 1) as *const u8),
                lpVerbW: PCWSTR((command as usize - 1) as *const u16),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            menu.InvokeCommand(&mut info as *mut _ as *const CMINVOKECOMMANDINFO)?;
            if let Ok(owner) = GetClipboardOwner() {
                let mut pid = 0;
                GetWindowThreadProcessId(owner, Some(&mut pid));
                if pid == std::process::id() {
                    let _ = OleFlushClipboard();
                }
            }
            // Format, device and Shell extension dialogs may be modeless in
            // this STA. Do not destroy their process immediately after Invoke.
            if !matches!(
                verb.as_str(),
                "copy" | "cut" | "copyaspath" | "pintohome" | "unpinfromhome"
            ) {
                pump_property_windows();
            }
        }
        Ok(())
    }
}
