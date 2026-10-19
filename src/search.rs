use crate::fs::Entry;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const RESULT_LIMIT: u32 = 20_000;
#[derive(Serialize, Deserialize)]
pub struct SearchResult {
    pub entries: Vec<Entry>,
    pub total: u32,
    pub engine: String,
}
pub fn sdk_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidates = [
        exe.parent()?.join("Everything64.dll"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".tools/everything-sdk/dll/Everything64.dll"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}
pub fn available() -> bool {
    use windows::{Win32::UI::WindowsAndMessaging::FindWindowW, core::w};
    sdk_path().is_some()
        && unsafe { FindWindowW(w!("EVERYTHING_TASKBAR_NOTIFICATION"), None) }.is_ok()
}
pub fn scoped_query(query: &str, folder: &Path, global: bool) -> String {
    if global {
        query.into()
    } else {
        format!("path:\"{}\" {}", folder.display(), query)
    }
}
pub fn installed_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for root in [
        windows_registry::CURRENT_USER,
        windows_registry::LOCAL_MACHINE,
    ] {
        if let Ok(key) =
            root.open("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\Everything.exe")
        {
            if let Ok(value) = key.get_string("") {
                candidates.push(PathBuf::from(value));
            }
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(
                PathBuf::from(base)
                    .join("Everything")
                    .join("Everything.exe"),
            );
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}
pub fn ensure_running(cancel: &Arc<AtomicBool>) -> Result<bool> {
    if available() {
        return Ok(true);
    }
    if sdk_path().is_none() {
        return Ok(false);
    }
    let Some(executable) = installed_path() else {
        return Ok(false);
    };
    let mut cmd = Command::new(executable);
    cmd.arg("-startup")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn().context("无法启动已安装的 Everything")?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(6) {
        if cancel.load(Ordering::Relaxed) {
            bail!("搜索已取消");
        }
        if available() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(false)
}

// A stuck SDK query must not retain a UI task or a forever-blocked worker.
pub fn query(
    query: &str,
    folder: &Path,
    global: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<SearchResult> {
    if !ensure_running(cancel)? {
        return fallback(query, folder, cancel);
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--everything-query", &scoped_query(query, folder, global)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let mut output = child.stdout.take().context("Everything stdout")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .by_ref()
            .take(32 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("Everything 查询取消或超时");
        }
        if let Some(status) = child.try_wait()? {
            let bytes = reader
                .join()
                .map_err(|_| anyhow::anyhow!("搜索进程读取失败"))??;
            if !status.success() {
                bail!("Everything 查询失败，请检查 Everything 是否允许 IPC、是否使用不同权限运行");
            }
            return serde_json::from_slice(&bytes).context("解析 Everything 结果失败");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn sdk_query(query: &str) -> Result<SearchResult> {
    use libloading::{Library, Symbol};
    let path = sdk_path().context("Everything64.dll 不可用，请运行 scripts/bootstrap.ps1")?;
    unsafe {
        let library = Library::new(path)?;
        let set_search: Symbol<unsafe extern "system" fn(*const u16)> =
            library.get(b"Everything_SetSearchW\0")?;
        let set_max: Symbol<unsafe extern "system" fn(u32)> =
            library.get(b"Everything_SetMax\0")?;
        let set_flags: Symbol<unsafe extern "system" fn(u32)> =
            library.get(b"Everything_SetRequestFlags\0")?;
        let run: Symbol<unsafe extern "system" fn(i32) -> i32> =
            library.get(b"Everything_QueryW\0")?;
        let error: Symbol<unsafe extern "system" fn() -> u32> =
            library.get(b"Everything_GetLastError\0")?;
        let count: Symbol<unsafe extern "system" fn() -> u32> =
            library.get(b"Everything_GetNumResults\0")?;
        let total: Symbol<unsafe extern "system" fn() -> u32> =
            library.get(b"Everything_GetTotResults\0")?;
        let full: Symbol<unsafe extern "system" fn(u32, *mut u16, u32) -> u32> =
            library.get(b"Everything_GetResultFullPathNameW\0")?;
        let folder: Symbol<unsafe extern "system" fn(u32) -> i32> =
            library.get(b"Everything_IsFolderResult\0")?;
        let get_size: Symbol<unsafe extern "system" fn(u32, *mut i64) -> i32> =
            library.get(b"Everything_GetResultSize\0")?;
        let get_date: Symbol<unsafe extern "system" fn(u32, *mut u64) -> i32> =
            library.get(b"Everything_GetResultDateModified\0")?;
        let get_attrs: Symbol<unsafe extern "system" fn(u32) -> u32> =
            library.get(b"Everything_GetResultAttributes\0")?;
        let query: Vec<u16> = query.encode_utf16().chain(Some(0)).collect();
        set_search(query.as_ptr());
        set_max(RESULT_LIMIT);
        set_flags(1 | 2 | 0x10 | 0x40 | 0x100);
        let reply_window = ReplyWindow::new(&library)?;
        if run(0) == 0 {
            bail!("Everything IPC 查询失败（SDK 错误 {}）", error());
        }
        reply_window.wait()?;
        let mut entries = Vec::new();
        for i in 0..count() {
            let mut buf = vec![0u16; 32768];
            let len = full(i, buf.as_mut_ptr(), buf.len() as u32) as usize;
            if len == 0 || len >= buf.len() {
                continue;
            }
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
            // Metadata access can block on offline volumes; defer it until preview/open.
            let directory = folder(i) != 0;
            let name = path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
            let kind = if directory {
                "文件夹".into()
            } else {
                path.extension()
                    .map(|v| format!("{} 文件", v.to_string_lossy().to_uppercase()))
                    .unwrap_or("文件".into())
            };
            let mut size = 0i64;
            let mut ticks = 0u64;
            get_size(i, &mut size);
            let modified = if get_date(i, &mut ticks) != 0 && ticks >= 116444736000000000 {
                std::time::UNIX_EPOCH.checked_add(Duration::from_nanos(
                    (ticks - 116444736000000000).saturating_mul(100),
                ))
            } else {
                None
            };
            let link = crate::fs::link_from_attributes(&path, get_attrs(i));
            entries.push(Entry {
                recycle: None,
                path,
                name,
                directory,
                link,
                hidden: get_attrs(i) & 2 != 0,
                system: get_attrs(i) & 4 != 0,
                size: size.max(0) as u64,
                total: None,
                free: None,
                modified,
                created: None,
                kind,
            });
        }
        Ok(SearchResult {
            entries,
            total: total(),
            engine: "Everything".into(),
        })
    }
}

type QueryReply = unsafe extern "system" fn(u32, usize, isize, u32) -> i32;
thread_local! {
    static QUERY_REPLY:std::cell::Cell<Option<QueryReply>>=const{std::cell::Cell::new(None)};
    static QUERY_READY:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
}
const REPLY_ID: u32 = 0x52555354;
unsafe extern "system" fn query_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    w: windows::Win32::Foundation::WPARAM,
    l: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{Foundation::LRESULT, UI::WindowsAndMessaging::*};
    if msg == WM_COPYDATA
        && QUERY_REPLY.with(|v| {
            v.get()
                .is_some_and(|f| unsafe { f(msg, w.0, l.0, REPLY_ID) != 0 })
        })
    {
        QUERY_READY.with(|v| v.set(true));
        return LRESULT(1);
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}
struct ReplyWindow(windows::Win32::Foundation::HWND);
impl ReplyWindow {
    unsafe fn new(library: &libloading::Library) -> Result<Self> {
        use windows::{
            Win32::{
                Foundation::HINSTANCE, System::LibraryLoader::GetModuleHandleW,
                UI::WindowsAndMessaging::*,
            },
            core::w,
        };
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = w!("RustExplorer.EverythingReply");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(query_proc),
                hInstance: HINSTANCE(module.0),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?;
            let owner = Self(hwnd);
            QUERY_REPLY.with(|v| {
                v.set(Some(
                    *library
                        .get::<QueryReply>(b"Everything_IsQueryReply\0")
                        .expect("SDK reply API"),
                ))
            });
            QUERY_READY.with(|v| v.set(false));
            let set_window: libloading::Symbol<unsafe extern "system" fn(isize)> =
                library.get(b"Everything_SetReplyWindow\0")?;
            let set_id: libloading::Symbol<unsafe extern "system" fn(u32)> =
                library.get(b"Everything_SetReplyID\0")?;
            set_window(hwnd.0 as isize);
            set_id(REPLY_ID);
            Ok(owner)
        }
    }
    fn wait(&self) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let started = Instant::now();
        while !QUERY_READY.with(|v| v.get()) {
            if started.elapsed() > Duration::from_secs(8) {
                bail!("Everything IPC 等待回复超时");
            }
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}
impl Drop for ReplyWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
        QUERY_REPLY.with(|v| v.set(None));
    }
}

// Honest fallback: current folder only; no recursive traversal of an unindexed disk.
fn fallback(query: &str, folder: &Path, cancel: &Arc<AtomicBool>) -> Result<SearchResult> {
    let (mut entries, _) = crate::fs::read_directory(folder, cancel)?;
    let needle = query.to_lowercase();
    entries.retain(|e| e.name.to_lowercase().contains(&needle));
    let total = entries.len() as u32;
    Ok(SearchResult {
        entries,
        total,
        engine: "当前文件夹名称筛选（Everything 不可用）".into(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_preserves_everything_syntax() {
        assert_eq!(
            scoped_query("ext:pdf", Path::new("D:\\My Files"), false),
            "path:\"D:\\My Files\" ext:pdf"
        );
        assert_eq!(scoped_query("ext:pdf", Path::new("D:\\x"), true), "ext:pdf");
    }
}
