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

/// Resolve portable-device PIDL paths as well as ordinary parsing names.
fn is_folder(path: &Path) -> bool {
    if crate::shell_folder::is_shell_path(path) {
        crate::shell_folder::is_folder(path)
    } else {
        path.is_dir()
    }
}
fn item(path: &Path) -> Result<IShellItem> {
    if crate::shell_folder::is_shell_path(path) {
        crate::shell_folder::item(path)
    } else {
        unsafe {
            SHCreateItemFromParsingName(PCWSTR(wide(path).as_ptr()), None).map_err(Into::into)
        }
    }
}
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
#[derive(Clone, Default)]
pub struct OperationControl {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    force: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl OperationControl {
    pub fn stop(&self, force: bool) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if force {
            self.force.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }
}
thread_local! {
    static OPERATION_CONTROL: RefCell<Option<OperationControl>> = const { RefCell::new(None) };
}
/// ⚠️ 警告（WARNING，勿回退）：**右键菜单绝对不要走"一次性 helper + 可取消"的请求路径**。
///
/// 菜单必须保留 `owner`（应用窗口）并留在**应用进程内**构建、显示（见
/// [`execute_menu_in_process`]）。一旦把菜单请求送去 `execute_one_shot`：
///
/// * 每次右键都会新起一个 `rust-explorer.exe` 子进程，并在其中**冷启动**重新加载
///   全部 Shell 扩展——实测首次 **≈250 ms，本机扩展多时可达 1 秒**；而进程内热路径
///   只要 **40–80 ms**。
/// * 菜单是"用户点一下就关"的交互，本来就不需要 `OperationControl` 的取消能力；
///   取消语义只对删除/复制/格式化这类长操作有意义。
/// * 菜单的 `owner` 一定不能在这里被清零（`*owner = 0`）。`owner = Some(hwnd)` 时
///   `execute_output` 才会走 [`execute_menu_in_process`]，否则会退回 helper，菜单
///   还会重新出现"被系统瞬间取消 / 抢激活 / 任务栏变灰"等 0.7.5 已修好的问题。
///
/// 历史教训：0.7.6 之后的一次"系统操作可取消"改动曾把菜单也并入 one-shot，导致右键
/// 菜单从 40–80 ms 退化到接近 1 秒。请勿再次合并这两类请求。
pub fn execute_controlled(request: Request, control: OperationControl) -> Result<String> {
    // Menus stay on the in-process path added in 0.7.5 (see the warning above).
    if is_menu_request(&request) {
        if cfg!(feature = "desktop") {
            debug_assert!(
                menu_owner(&request).is_some(),
                "菜单请求必须携带 owner 才能在应用进程内显示；\
                 若这里失败，说明有人又把 owner 清零或漏传了（会让右键菜单退化到 ~1s）"
            );
        }
        menu_trace("menu: in-process (cancellable operations keep their helper)");
        return execute_output(request);
    }
    // Every other user-facing Shell operation is killable without terminating a GUI
    // thread. A helper owns its popup; never create cross-process child windows.
    OPERATION_CONTROL.with(|slot| *slot.borrow_mut() = Some(control));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            OPERATION_CONTROL.with(|slot| *slot.borrow_mut() = None);
        }
    }
    let _reset = Reset;
    execute_one_shot(request)
}
pub fn operation_label(request: &Request) -> String {
    let name = match request {
        Request::Menu { .. } | Request::ThemedMenu { .. } | Request::ComputerMenu { .. } => {
            "系统右键菜单"
        }
        Request::Properties { .. } | Request::PropertiesMany { .. } => "属性",
        Request::Delete { .. } => "移到回收站",
        Request::PermanentDelete { .. } => "永久删除",
        Request::Rename { .. } => "重命名",
        Request::RecordCreated { .. } => "记录新建项目",
        Request::Share { .. } | Request::AdvancedShare { .. } | Request::SharingAccess { .. } => {
            "共享设置"
        }
        Request::InvokeVerb { verb, .. } if verb.eq_ignore_ascii_case("format") => "打开格式化窗口",
        Request::InvokeVerb { verb, .. } => return format!("系统命令：{verb}"),
        _ => "系统操作",
    };
    name.into()
}

unsafe fn close_process_windows(pid: u32) {
    unsafe extern "system" fn close(hwnd: HWND, param: LPARAM) -> BOOL {
        let mut current = hwnd;
        for _ in 0..16 {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(current, Some(&mut pid));
            }
            if pid == param.0 as u32 {
                // Some Windows dialogs are hosted by dllhost but owned by the
                // helper's window. Request cancellation there too; do not kill
                // a shared system host process.
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                break;
            }
            let Ok(owner) = (unsafe { GetWindow(current, GW_OWNER) }) else {
                break;
            };
            if owner.is_invalid() || owner == current {
                break;
            }
            current = owner;
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(close), LPARAM(pid as isize));
    }
}
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
    EmptyRecycleBin,
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
    /// Opens the classic Sharing property page (Advanced Sharing).
    AdvancedShare {
        path: PathBuf,
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
        #[serde(default)]
        can_paste: bool,
        /// The single selected folder is currently pinned to Windows Quick Access,
        /// so the Shell's `pintohome` entry must act as an unpin.
        #[serde(default)]
        quick_access_pinned: bool,
        /// HWND of the application window that owns the menu. When non-zero the
        /// menu is shown inside the application process, so the app itself stays
        /// the active process instead of a helper stealing activation.
        #[serde(default)]
        owner: u64,
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
        #[serde(default)]
        owner: u64,
    },
    AddNetworkLocation,
    /// Invoke one Shell verb through the Shell's own command (helper process).
    /// Used for verbs that show a modal dialog and block until it closes.
    InvokeVerb {
        paths: Vec<PathBuf>,
        verb: String,
    },
    RenameComputer {
        dark: bool,
        /// Collected by the inline editor; `None` falls back to the native prompt.
        #[serde(default)]
        name: Option<String>,
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
/// Context menus are answered by a persistent STA helper. A cold
/// `QueryContextMenu` costs ~250 ms because every registered Shell extension has
/// to be loaded into a fresh process; the same call costs ~44 ms once they are
/// warm. Every other request keeps the isolated one-shot helper so a misbehaving
/// extension can never outlive a single operation.
static MENU_HOST: std::sync::Mutex<Option<MenuHost>> = std::sync::Mutex::new(None);
/// True while a native menu request is in flight. The hover card is a topmost
/// window; it must not be created or shown while a menu is on screen.
static MENU_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn menu_active() -> bool {
    MENU_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}
const MENU_HOST_DONE: &str = "@@done";
const MENU_HOST_ERROR: &str = "@@err:";

struct MenuHost {
    child: std::process::Child,
    input: std::process::ChildStdin,
    replies: std::sync::mpsc::Receiver<String>,
}
impl MenuHost {
    fn start() -> Result<Self> {
        use std::{io::BufRead, os::windows::process::CommandExt, process::Stdio};
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .arg("--shell-server")
            .arg(std::process::id().to_string())
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().context("菜单宿主输入不可用")?;
        let stdout = child.stdout.take().context("菜单宿主输出不可用")?;
        let (sender, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            replies,
        })
    }
    fn send(&mut self, payload: &str) -> Result<()> {
        use std::io::Write;
        writeln!(self.input, "{payload}")?;
        self.input.flush()?;
        Ok(())
    }
    /// Collect one response frame: everything up to the `@@done` marker.
    fn response(&mut self) -> Result<HostFrame> {
        self.response_within(None)
    }
    /// `Failed` means the host answered but refused the request: the process is
    /// healthy, so the caller must not treat it as a crash and rebuild the menu.
    fn response_within(&mut self, limit: Option<std::time::Duration>) -> Result<HostFrame> {
        let mut collected = String::new();
        loop {
            let line = match limit {
                Some(limit) => self
                    .replies
                    .recv_timeout(limit)
                    .map_err(|error| anyhow::anyhow!("菜单宿主无响应：{error}"))?,
                None => self
                    .replies
                    .recv()
                    .map_err(|_| anyhow::anyhow!("菜单宿主已退出"))?,
            };
            if line == MENU_HOST_DONE {
                return Ok(HostFrame::Done(collected.trim().to_string()));
            }
            if let Some(message) = line.strip_prefix(MENU_HOST_ERROR) {
                return Ok(HostFrame::Failed(message.to_string()));
            }
            if !line.is_empty() {
                if !collected.is_empty() {
                    collected.push('\n');
                }
                collected.push_str(&line);
            }
        }
    }
}
impl Drop for MenuHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn is_menu_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Menu { .. }
            | Request::ThemedMenu { .. }
            | Request::ComputerMenu { .. }
            // Read-only enumeration, also used to warm the host at startup.
            | Request::InspectMenu { .. }
    )
}
/// One response frame from the menu host.
enum HostFrame {
    Done(String),
    Failed(String),
}
/// Append a line to `%TEMP%\rust-explorer-menu.log` when
/// `RUST_EXPLORER_MENU_TRACE` is set. Diagnostics only, off by default.
pub fn menu_trace(message: impl std::fmt::Display) {
    if std::env::var_os("RUST_EXPLORER_MENU_TRACE").is_none() {
        return;
    }
    use std::io::Write;
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    let path = std::env::temp_dir().join("rust-explorer-menu.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "[{} +{:>6}ms] {}",
            std::process::id(),
            start.elapsed().as_millis(),
            message
        );
    }
}
/// Load the Shell's context-menu handlers before the first right-click. A cold
/// `QueryContextMenu` costs ~250 ms, so warming at startup keeps the first menu
/// as fast as the ones after it.
pub fn prewarm_menu_host() {
    let Ok(mut guard) = MENU_HOST.lock() else {
        return;
    };
    if guard.is_none() {
        *guard = MenuHost::start().ok();
    }
    let Some(active) = guard.as_mut() else {
        return;
    };
    menu_trace("prewarm: begin");
    let warmup = r#"{"InspectMenu":{"paths":[],"folder":"C:\\Windows","computer":false,"host_commands":false}}"#;
    let warmed = active.send(warmup).is_ok()
        && matches!(
            active.response_within(Some(std::time::Duration::from_secs(15))),
            Ok(HostFrame::Done(_))
        );
    if warmed {
        menu_trace("prewarm: done");
    } else {
        menu_trace("prewarm: failed, host dropped");
        *guard = None;
    }
}
/// Load the Shell's context-menu handlers into this process before the first
/// right-click. The first `QueryContextMenu` on a cold process loads every
/// registered shell extension (≈250 ms–1 s on machines with many of them); the
/// menu window itself is built in-process, so warming has to happen here rather
/// than in a helper process.
pub fn prewarm_in_process_menu() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::thread::Builder::new()
            .name("shell-menu-prewarm".into())
            .spawn(|| {
                let started = std::time::Instant::now();
                let exe = std::env::current_exe().ok();
                let folder = exe
                    .as_ref()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
                    .unwrap_or_else(std::env::temp_dir);
                // A selected file warms the file-type handlers, the folder itself
                // warms the background menu; both share the same extension modules.
                let warmups: Vec<(Vec<PathBuf>, PathBuf)> = exe
                    .map(|exe| vec![(vec![exe], folder.clone()), (Vec::new(), folder.clone())])
                    .unwrap_or_else(|| vec![(Vec::new(), folder.clone())]);
                for (paths, folder) in warmups {
                    let result = on_menu_sta(move || {
                        context_menu_options(
                            &paths, &folder, 0, 0, None, false, false, true, None, false, None,
                        )
                    });
                    menu_trace(format!(
                        "prewarm: in-process menu ok={} elapsed={:?}",
                        result.is_ok(),
                        started.elapsed()
                    ));
                }
            })
            .ok();
    });
}
/// Exit this helper as soon as its parent process disappears.
///
/// A menu host can sit inside `TrackPopupMenuEx`, where it cannot observe a
/// closed stdin pipe, so without this it would outlive its parent as an orphan
/// holding a live modal menu. Waiting on the parent handle turns that into a
/// clean process exit.
pub fn watch_parent_process(pid: u32) {
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Threading::{
            INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };
        if let Ok(parent) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(parent, INFINITE);
            let _ = windows::Win32::Foundation::CloseHandle(parent);
            std::process::exit(0);
        }
    });
}

/// Pointer travel allowed between the right-button press and release before the
/// gesture counts as a right-drag instead of a context click. Explorer uses the
/// system drag rectangle (~4 px); a menu needs more slack because a hand jitters
/// a few pixels during an ordinary click.
const CONTEXT_MENU_SLOP: f32 = 12.0;

/// Wait for the invoking button to be released, then let the app dispatch that
/// button-up before the menu appears.
///
/// Two races are handled here:
/// * a menu that appears before the release receives the pending button-up and
///   dismisses itself on the spot;
/// * polling `GetAsyncKeyState` reports the release a moment before the app's
///   window processes `WM_RBUTTONUP`, so a short settle is required.
///
/// The pointer travel that decides "click or right-drag" is sampled **between the
/// press and the release only**. Moving the pointer after the button is released
/// (which is exactly what happens while the menu is still being built) must never
/// suppress the menu.
///
/// Returns `false` when that press-to-release travel exceeds
/// [`CONTEXT_MENU_SLOP`]: the gesture was a right-drag and no menu must be shown.
fn wait_for_context_gesture(hwnd: HWND, _x: i32, _y: i32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_RBUTTON};
    fn cursor() -> Option<windows::Win32::Foundation::POINT> {
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe { GetCursorPos(&mut point) }.is_ok().then_some(point)
    }
    let before = cursor();
    let mut waited = false;
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_millis(700);
    loop {
        let down = (unsafe { GetAsyncKeyState(VK_RBUTTON.0 as i32) } as u16 & 0x8000) != 0;
        if !down {
            break;
        }
        waited = true;
        if std::time::Instant::now() >= deadline {
            menu_trace("host: button still down after 700 ms, showing anyway");
            break;
        }
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if waited {
        menu_trace(format!(
            "host: waited {} ms for the button release",
            start.elapsed().as_millis()
        ));
    }
    // Sampled at the release, before the settle: later movement is irrelevant.
    if let (Some(before), Some(after)) = (before, cursor()) {
        let dx = (after.x - before.x) as f32;
        let dy = (after.y - before.y) as f32;
        let travelled = (dx * dx + dy * dy).sqrt();
        if travelled > CONTEXT_MENU_SLOP {
            menu_trace(format!(
                "host: pointer travelled {travelled:.0} px between press and release, treated as a right-drag"
            ));
            return false;
        }
    }
    // Always let the owning window finish its own mouse-up handling before the menu
    // appears: GPUI releases mouse capture and runs its dispatch tail there, and a
    // menu shown in the middle of that can be dismissed straight away.
    std::thread::sleep(std::time::Duration::from_millis(30));
    true
}

/// Claim the foreground window for the menu. The parent's
/// `AllowSetForegroundWindow` is a one-shot grant that user input can void, so
/// when the plain call does not take, fall back to the documented
/// `AttachThreadInput` technique. A menu whose owner is not foreground is drawn
/// but not interactive: the first click dismisses it.
fn claim_menu_foreground(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if GetForegroundWindow() == hwnd {
            return;
        }
        let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let current_thread = GetCurrentThreadId();
        if foreground_thread == 0 || foreground_thread == current_thread {
            return;
        }
        if AttachThreadInput(current_thread, foreground_thread, true).as_bool() {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetActiveWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(current_thread, foreground_thread, false);
        }
    }
}

/// `--shell-server`: long-lived STA helper. One JSON request per stdin line,
/// answered with a single `@@done`/`@@err:` marker line after any command output.
pub fn serve_menu_requests() -> Result<()> {
    use std::io::{BufRead, Write};
    // Hold one STA reference for the whole session so Shell handlers stay loaded.
    let _com = Com::new()?;
    menu_trace("host: serving");
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                writeln!(out, "{MENU_HOST_ERROR}无效请求：{error}")?;
                out.flush()?;
                continue;
            }
        };
        if !is_menu_request(&request) {
            writeln!(out, "{MENU_HOST_ERROR}该服务只处理菜单请求")?;
            out.flush()?;
            continue;
        }
        match dispatch(request) {
            Ok(()) => writeln!(out, "{MENU_HOST_DONE}")?,
            Err(error) => {
                menu_trace(format!("host: request failed: {error:#}"));
                writeln!(out, "{MENU_HOST_ERROR}{error:#}")?
            }
        }
        out.flush()?;
    }
    Ok(())
}
fn execute_menu_output(request: Request) -> Result<String> {
    MENU_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    struct ClearMenuActive;
    impl Drop for ClearMenuActive {
        fn drop(&mut self) {
            MENU_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _clear = ClearMenuActive;
    let payload = serde_json::to_string(&request)?;
    let mut guard = MENU_HOST
        .lock()
        .map_err(|_| anyhow::anyhow!("菜单宿主不可用"))?;
    if guard.is_none() {
        *guard = MenuHost::start().ok();
        menu_trace(format!(
            "host started: {:?}",
            guard.as_ref().map(|host| host.child.id())
        ));
    }
    if let Some(active) = guard.as_mut() {
        unsafe {
            let _ = AllowSetForegroundWindow(active.child.id());
        }
        if active.send(&payload).is_ok() {
            match active.response() {
                Ok(HostFrame::Done(response)) => {
                    return Ok(response.trim_end().to_string());
                }
                // The host answered: keep it warm and surface the failure. Retrying
                // here would build the very same menu a second time.
                Ok(HostFrame::Failed(message)) => {
                    menu_trace(format!("host refused the request: {message}"));
                    bail!("{message}");
                }
                Err(error) => menu_trace(format!("host lost: {error}")),
            }
        }
    }
    menu_trace("falling back to a one-shot helper");
    *guard = None;
    drop(guard);
    execute_one_shot(request)
}
pub fn execute_output(request: Request) -> Result<String> {
    if is_menu_request(&request) {
        // ⚠️ 不要删除这个分支：菜单必须在应用进程内显示（见 execute_controlled 的警告）。
        // 只有没有 owner 的旧式/CLI 调用才会退回 helper。
        if menu_owner(&request).is_some() {
            return execute_menu_in_process(request);
        }
        return execute_menu_output(request);
    }
    execute_one_shot(request)
}

fn menu_owner(request: &Request) -> Option<HWND> {
    match request {
        Request::ThemedMenu { owner, .. } | Request::ComputerMenu { owner, .. } => {
            owner_hwnd(*owner)
        }
        _ => None,
    }
}

/// GPUI's shared workers may already be MTA. Never change their apartment or
/// ignore RPC_E_CHANGED_MODE: create the menu and all its COM objects on a fresh STA.
fn execute_menu_in_process(request: Request) -> Result<String> {
    on_menu_sta(move || execute_menu_on_sta(request))
}

fn on_menu_sta<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    std::thread::Builder::new()
        .name("shell-menu-sta".into())
        .spawn(move || {
            let _com = Com::new()?;
            operation()
        })
        .context("Could not start Shell menu STA")?
        .join()
        .map_err(|_| anyhow::anyhow!("Shell menu STA panicked"))?
}

fn execute_menu_on_sta(request: Request) -> Result<String> {
    MENU_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    struct ClearMenuActive;
    impl Drop for ClearMenuActive {
        fn drop(&mut self) {
            MENU_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _clear = ClearMenuActive;
    let output = match request {
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
            can_paste,
            quick_access_pinned,
            owner,
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
                folder: false,
                folder_insert: 0,
                can_paste,
            }),
            quick_access_pinned,
            owner_hwnd(owner),
        ),
        Request::ComputerMenu {
            x,
            y,
            dark,
            expanded,
            owner,
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
            false,
            owner_hwnd(owner),
        ),
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
                folder: false,
                folder_insert: 0,
                can_paste: false,
            }),
            false,
            None,
        ),
        Request::Menu {
            paths,
            folder,
            x,
            y,
        } => context_menu_options(
            &paths,
            &folder,
            x,
            y,
            Some(crate::theme::Theme::system().dark),
            false,
            false,
            false,
            None,
            false,
            None,
        ),
        _ => bail!("不是菜单请求"),
    }?;
    // The caller matches on the raw line (`strip_prefix("command:")`), so the
    // trailing newline must not be part of the result.
    Ok(output.trim_end().to_string())
}
/// Run one request in a short-lived helper process.
///
/// ⚠️ 警告（WARNING）：**不要把右键菜单带进这里**。每次调用都会创建新进程并冷加载
/// Shell 扩展，菜单从进程内的 40–80 ms 退化到 ≈250 ms（扩展多时接近 1 秒）。
/// 菜单只在没有 owner 的旧式路径下才会经 [`execute_menu_output`] 兜底到这里；
/// 有 owner 时必须走 [`execute_menu_in_process`]。
fn execute_one_shot(request: Request) -> Result<String> {
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
    struct OwnedChild(std::process::Child);
    impl std::ops::Deref for OwnedChild {
        type Target = std::process::Child;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl std::ops::DerefMut for OwnedChild {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let mut child = OwnedChild(cmd.spawn()?);
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
    let control = OPERATION_CONTROL.with(|slot| slot.borrow().clone());
    let mut stopped = false;
    let status = if pin_deadline || control.is_some() {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if let Some(control) = &control {
                if control.stopping() && !stopped {
                    stopped = true;
                    unsafe {
                        close_process_windows(child.id());
                    }
                }
                if control.force.load(std::sync::atomic::Ordering::Relaxed) {
                    child.kill().context("无法强行停止系统操作进程")?;
                    break child.wait()?;
                }
            }
            if pin_deadline && start.elapsed() >= std::time::Duration::from_secs(5) {
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
    anyhow::ensure!(!stopped, "系统操作已停止；可能已完成部分操作，请刷新确认");
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

    /// 回归测试：右键菜单必须留在应用进程内（见 `execute_controlled` 的警告）。
    ///
    /// 菜单一旦变成"一次性 helper + 可取消"请求，每次右键都要新起进程并冷加载
    /// Shell 扩展，实测 ≈250 ms 起、扩展多时接近 1 秒；进程内热路径是 40–80 ms。
    #[test]
    fn context_menus_keep_an_owner_and_stay_out_of_the_one_shot_helper() {
        let path = PathBuf::from("C:\\Windows\\notepad.exe");
        let folder = PathBuf::from("C:\\Windows");
        let themed = Request::ThemedMenu {
            paths: vec![path.clone()],
            folder: folder.clone(),
            x: 0,
            y: 0,
            dark: false,
            view: 5,
            sort: "name".into(),
            descending: false,
            group: String::new(),
            undo: false,
            redo: false,
            can_paste: false,
            quick_access_pinned: false,
            owner: 0x1234,
        };
        assert!(is_menu_request(&themed), "ThemedMenu 必须被识别为菜单请求");
        assert!(
            menu_owner(&themed).is_some(),
            "菜单请求必须携带 owner；owner 丢失会让菜单退回一次性 helper（右键菜单 ≈1s）"
        );
        let computer = Request::ComputerMenu {
            x: 0,
            y: 0,
            dark: false,
            expanded: false,
            owner: 0x1234,
        };
        assert!(is_menu_request(&computer));
        assert!(menu_owner(&computer).is_some());
        // 非菜单请求没有 owner，仍走一次性 helper（可取消）。
        assert!(menu_owner(&Request::Open { path }).is_none());
    }
    #[test]
    fn format_targets_are_exact_drive_roots_and_failures_are_not_success() {
        for value in ["c", "C:", "c:\\", "C:/"] {
            assert_eq!(format_drive_letter(value).unwrap(), 'C');
        }
        for value in [
            "",
            "hello",
            "C:\\data",
            "\\\\server\\share",
            "shell:ComputerFolder",
            "1:",
        ] {
            assert!(format_drive_letter(value).is_err(), "{value}");
        }
        assert!(format_result(SHFMT_CANCEL.0).is_ok());
        assert!(format_result(0).is_ok());
        assert!(format_result(SHFMT_ERROR.0).is_err());
        assert!(format_result(SHFMT_NOFORMAT.0).is_err());
    }
    #[test]
    fn native_menu_uses_fresh_sta_when_caller_is_mta() {
        std::thread::spawn(|| {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .unwrap();
            struct Mta;
            impl Drop for Mta {
                fn drop(&mut self) {
                    unsafe { CoUninitialize() }
                }
            }
            let _mta = Mta;
            let caller = std::thread::current().id();
            on_menu_sta(move || {
                assert_ne!(caller, std::thread::current().id());
                let mut apartment = APTTYPE_CURRENT;
                let mut qualifier = APTTYPEQUALIFIER_NONE;
                unsafe { CoGetApartmentType(&mut apartment, &mut qualifier)? };
                assert!(apartment == APTTYPE_STA || apartment == APTTYPE_MAINSTA);
                // Exercise the real Shell context-menu creation path without
                // displaying a popup or executing any command.
                let output = execute_menu_on_sta(Request::InspectMenu {
                    paths: vec![],
                    folder: std::env::temp_dir(),
                    computer: false,
                    host_commands: true,
                })?;
                assert!(!output.is_empty());
                Ok(())
            })
            .unwrap();
            let mut apartment = APTTYPE_CURRENT;
            let mut qualifier = APTTYPEQUALIFIER_NONE;
            unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) }.unwrap();
            assert_eq!(apartment, APTTYPE_MTA);
        })
        .join()
        .unwrap();
    }
    #[test]
    #[ignore = "Requires an interactive Windows Shell; deletes only its own empty temporary fixture"]
    fn native_delete_survives_unsupported_progress_caption() {
        let parent = std::env::temp_dir().join(format!(
            "re-native-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&parent).unwrap();
        let recycled = parent.join("recycle-fixture");
        let permanent = parent.join("permanent-fixture");
        std::fs::create_dir(&recycled).unwrap();
        std::fs::create_dir(&permanent).unwrap();
        dispatch(Request::Delete {
            paths: vec![recycled.clone()],
        })
        .unwrap();
        assert!(!recycled.exists());
        dispatch(Request::PermanentDelete {
            paths: vec![permanent.clone()],
        })
        .unwrap();
        assert!(!permanent.exists());
        std::fs::remove_dir(&parent).unwrap();
    }
    #[test]
    fn start_pin_restrictions_are_reported_without_requesting_elevation() {
        assert!(is_start_pin_verb("pintostartscreen"));
        assert!(is_start_pin_verb("startunpin"));
        assert!(!is_start_pin_verb("pintohome"));
        assert!(!is_start_pin_verb("runas"));
        let message = start_pin_failure(windows::core::Error::from_hresult(
            windows::core::HRESULT(0x80070005u32 as i32),
        ))
        .to_string();
        assert!(message.contains("0x80070005"));
        assert!(message.contains("Windows 资源管理器"));
        assert!(message.contains("未修改权限"));
    }
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
        Request::EmptyRecycleBin => unsafe {
            // Keep Windows confirmation, progress and errors enabled.
            SHEmptyRecycleBinW(None, PCWSTR::null(), 0).map_err(Into::into)
        },
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
        Request::RenameVolume { path, label } => {
            crate::volume_label::rename_with_elevation(&path, &label)
        }
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
            let destination_shell = crate::shell_folder::is_shell_path(&destination);
            let target = if destination_shell {
                None
            } else {
                anyhow::ensure!(destination.is_dir(), "目标文件夹不存在");
                Some(std::fs::canonicalize(&destination)?)
            };
            if let Some(target) = &target {
                // Resolve junctions before rejecting a drop onto itself or a descendant.
                // Compare canonical paths component-wise: a drive root canonicalizes to
                // `\\?\C:\` (trailing separator), so string prefixes silently miss it.
                for path in paths
                    .iter()
                    .filter(|path| !crate::shell_folder::is_shell_path(path))
                {
                    let source = std::fs::canonicalize(path)?;
                    anyhow::ensure!(
                        !target.starts_with(&source),
                        "不能将项目放入自身或其子文件夹"
                    );
                }
            }
            let shell_namespace = destination_shell
                || paths
                    .iter()
                    .any(|path| crate::shell_folder::is_shell_path(path));
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = if shell_namespace {
                None
            } else {
                Some(crate::file_history::Recorder::attach(&op, &paths)?)
            };
            op.SetOperationFlags(if shell_namespace {
                FILEOPERATION_FLAGS(FOF_NOCONFIRMATION.0)
            } else {
                FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0)
            })?;
            let dst: IShellItem = item(&destination)?;
            let target_volume = target.as_ref().and_then(|path| volume_identity(path));
            for path in paths {
                let source_shell = crate::shell_folder::is_shell_path(&path);
                if mode == TransferMode::Link {
                    anyhow::ensure!(
                        !destination_shell && !source_shell,
                        "无法在便携设备上创建快捷方式"
                    );
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
                    if let Some(history) = &_history {
                        history.created(&link_path);
                    }
                    continue;
                }
                let src: IShellItem = item(&path)?;
                let moving = mode == TransferMode::Move
                    || (mode == TransferMode::Auto
                        && !copy_only
                        && !source_shell
                        && !destination_shell
                        && target_volume.is_some()
                        && std::fs::canonicalize(&path)
                            .ok()
                            .and_then(|path| volume_identity(&path))
                            == target_volume);
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
        Request::AdvancedShare { path } => advanced_share(&path),
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
            can_paste,
            quick_access_pinned,
            owner,
        } => {
            let output = context_menu_options(
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
                    folder: false,
                    folder_insert: 0,
                    can_paste,
                }),
                quick_access_pinned,
                owner_hwnd(owner),
            )?;
            print!("{output}");
            Ok(())
        }
        Request::ComputerMenu {
            x,
            y,
            dark,
            expanded,
            owner,
        } => {
            let output = context_menu_options(
                &[],
                Path::new("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
                x,
                y,
                Some(dark),
                true,
                expanded,
                false,
                None,
                false,
                owner_hwnd(owner),
            )?;
            print!("{output}");
            Ok(())
        }
        Request::AddNetworkLocation => add_network_location(),
        Request::InvokeVerb { paths, verb } => invoke_verb(&paths, &verb),
        Request::RenameComputer { dark, name } => namespace_rename::computer(dark, name),
        Request::ComputerName => {
            println!("computer-name:{}", namespace_rename::computer_name()?);
            Ok(())
        }
        Request::InspectMenu {
            paths,
            folder,
            computer,
            host_commands,
        } => {
            let output = context_menu_options(
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
                    folder: false,
                    folder_insert: 0,
                    can_paste: false,
                }),
                false,
                None,
            )?;
            print!("{output}");
            Ok(())
        }
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
            let shell_namespace = crate::shell_folder::is_shell_path(&destination)
                || paths
                    .iter()
                    .any(|path| crate::shell_folder::is_shell_path(path));
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .context("创建文件操作")?;
            // Portable-device operations have no meaningful filesystem undo
            // record; let the Shell perform the copy/delete directly.
            let _history = if shell_namespace {
                None
            } else {
                Some(crate::file_history::Recorder::attach(&op, &paths)?)
            };
            let flags = if shell_namespace {
                FILEOPERATION_FLAGS(FOF_NOCONFIRMATION.0)
            } else {
                FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0)
            };
            op.SetOperationFlags(flags).context("设置文件操作标志")?;
            let dst: IShellItem = item(&destination).context("解析目标文件夹")?;
            for path in paths {
                let src: IShellItem = item(&path).context("解析源文件")?;
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
            op.SetOperationFlags(FILEOPERATION_FLAGS(FOF_NOCONFIRMATION.0))?;
            // SetProgressMessage is documented as not implemented. Keep the
            // system-provided Unicode progress caption instead.
            for path in paths {
                if !crate::shell_folder::is_shell_path(&path) {
                    anyhow::ensure!(
                        path.is_absolute() && path.parent().is_some(),
                        "Refuse permanent deletion of a volume root"
                    );
                }
                let item = item(&path)?;
                op.DeleteItem(&item, None)?;
            }
            op.PerformOperations()?;
            if op.GetAnyOperationsAborted()?.as_bool() {
                bail!("永久删除已取消");
            }
            Ok(())
        },
        Request::Delete { paths } => unsafe {
            let shell_namespace = paths
                .iter()
                .any(|path| crate::shell_folder::is_shell_path(path));
            let recycle = !shell_namespace && can_recycle(&paths);
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = if recycle {
                Some(crate::file_history::Recorder::attach(&op, &paths)?)
            } else {
                None
            };
            let flags = if shell_namespace || !recycle {
                FILEOPERATION_FLAGS(FOF_NOCONFIRMATION.0 | FOF_WANTNUKEWARNING.0)
            } else {
                FILEOPERATION_FLAGS(
                    FOF_ALLOWUNDO.0
                        | FOFX_ADDUNDORECORD.0
                        | FOFX_RECYCLEONDELETE.0
                        | FOF_WANTNUKEWARNING.0,
                )
            };
            op.SetOperationFlags(flags)?;
            for path in paths {
                let item = item(&path)?;
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
            let shell_namespace = crate::shell_folder::is_shell_path(&path);
            let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
            let _history = if shell_namespace {
                None
            } else {
                Some(crate::file_history::Recorder::attach(
                    &op,
                    std::slice::from_ref(&path),
                )?)
            };
            op.SetOperationFlags(if shell_namespace {
                FILEOPERATION_FLAGS(FOF_NOCONFIRMATION.0)
            } else {
                FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0)
            })?;
            let item = item(&path)?;
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
    if crate::shell_folder::is_shell_path(path) {
        return crate::shell_folder::open(path);
    }
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
        // INVOKEIDLIST lets portable-device items open through their Shell
        // namespace handler instead of being treated as string paths.
        fMask: SEE_MASK_NOASYNC | SEE_MASK_INVOKEIDLIST,
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
    if crate::shell_folder::is_shell_path(path) {
        crate::shell_folder::properties(path)?;
        unsafe {
            pump_property_windows();
        }
        return Ok(());
    }
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

pub fn advanced_share(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.is_absolute() && path.is_dir(),
        "请选择要设置共享的文件夹或驱动器"
    );
    unsafe {
        if SHObjectProperties(
            None,
            SHOP_FILEPATH,
            PCWSTR(wide(path).as_ptr()),
            w!("Sharing"),
        )
        .as_bool()
        {
            pump_property_windows();
            Ok(())
        } else {
            bail!("Windows 无法打开高级共享设置")
        }
    }
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
pub(crate) unsafe fn pump_property_windows() {
    unsafe { pump_property_windows_scoped(false) }
}

unsafe fn pump_property_windows_scoped(current_thread_only: bool) {
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
        let param = LPARAM(&mut windows as *mut _ as isize);
        if current_thread_only {
            // The application's main window must not keep a menu worker alive.
            let _ = unsafe {
                EnumThreadWindows(
                    windows::Win32::System::Threading::GetCurrentThreadId(),
                    Some(count),
                    param,
                )
            };
        } else {
            let _ = unsafe { EnumWindows(Some(count), param) };
        }
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
        // Release the COM string before any early return: `to_string` can fail on
        // malformed UTF-16 and would otherwise leak the CoTaskMem allocation.
        let text = p.to_string();
        CoTaskMemFree(Some(p.0.cast()));
        Some(PathBuf::from(text.ok()?))
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
/// A drive without a usable recycle bin (many USB sticks, network shares)
/// must be treated as permanent deletion before the operation starts.
pub fn can_recycle(paths: &[PathBuf]) -> bool {
    use windows::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
    unsafe {
        for path in paths {
            if crate::shell_folder::is_shell_path(path) {
                return false;
            }
            let mut root = [0u16; 32768];
            if windows::Win32::Storage::FileSystem::GetVolumePathNameW(
                PCWSTR(wide(path).as_ptr()),
                &mut root,
            )
            .is_err()
            {
                return false;
            }
            let mut info = SHQUERYRBINFO {
                cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
                ..Default::default()
            };
            if SHQueryRecycleBinW(PCWSTR(root.as_ptr()), &mut info).is_err() {
                return false;
            }
        }
    }
    true
}
pub fn has_file_clipboard() -> bool {
    unsafe { IsClipboardFormatAvailable(15).is_ok() }
}
pub fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
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
    // Trace every window message while a menu is up: a dismissal that is not our
    // own `EndMenu` shows as an activation/focus/capture change here.
    const NOISE: [u32; 9] = [
        WM_MOUSEMOVE,
        WM_PAINT,
        WM_NCHITTEST,
        WM_ERASEBKGND,
        WM_SETCURSOR,
        WM_NCCALCSIZE,
        WM_WINDOWPOSCHANGING,
        WM_WINDOWPOSCHANGED,
        WM_GETMINMAXINFO,
    ];
    if !NOISE.contains(&msg) {
        menu_trace(format!("host: msg 0x{msg:04x} w=0x{:x} l=0x{:x}", w.0, l.0));
    }
    if msg == WM_CAPTURECHANGED {
        // lParam is the window that took the capture away: name its process.
        let thief = HWND(l.0 as *mut _);
        let mut pid = 0u32;
        let mut class = [0u16; 128];
        let length = unsafe {
            let _ = GetWindowThreadProcessId(thief, Some(&mut pid));
            GetClassNameW(thief, &mut class) as usize
        };
        menu_trace(format!(
            "host: capture taken by hwnd=0x{:x} pid={pid} class={}",
            l.0,
            String::from_utf16_lossy(&class[..length])
        ));
    }
    if matches!(
        msg,
        WM_RBUTTONDOWN | WM_RBUTTONUP | WM_LBUTTONDOWN | WM_LBUTTONUP
    ) {
        menu_trace(format!(
            "host: mouse message 0x{msg:04x} while the menu is up"
        ));
    }
    if msg == WM_CANCELMODE || (msg == WM_ACTIVATEAPP && w.0 == 0) {
        menu_trace(format!("host: window message 0x{msg:04x} ends the menu"));
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
fn menu_pidl(path: &Path) -> Result<Pidl> {
    let pointer = if crate::shell_folder::is_shell_path(path) {
        let bytes = crate::shell_folder::pidl(path)?;
        unsafe { ILClone(bytes.as_ptr().cast()) }
    } else {
        unsafe { ILCreateFromPathW(PCWSTR(wide(path).as_ptr())) }
    };
    anyhow::ensure!(!pointer.is_null(), "无法读取 Shell 菜单项目");
    Ok(Pidl(pointer))
}
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

/// Invoke one Shell verb through the Shell's registered command. Runs in a helper
/// process: verbs such as format/print/share show a modal dialog and block the
/// calling thread until it closes, which must never happen inside the application.
/// Show the Shell's format dialog for `drive` (a letter or `D:\`). Runs in an
/// elevated helper: `SHFormatDrive` formats from the calling process, so it needs
/// administrator rights, and its dialog is modal until the user closes it.
pub fn format_drive(drive: &str) -> Result<()> {
    let letter = format_drive_letter(drive)?;
    let index = (letter as u8 - b'A') as u32;
    let _com = Com::new()?;
    // SHFormatDrive requires a non-NULL parent, even inside a helper process.
    let module = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None)? };
    let owner = MenuWindow(unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("Rust Explorer"),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1,
            1,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?
    });
    // Returns when the dialog closes (or immediately with SHFMT_ERROR on failure).
    let result = unsafe {
        windows::Win32::UI::Shell::SHFormatDrive(
            owner.0,
            index,
            windows::Win32::UI::Shell::SHFMT_ID_DEFAULT,
            windows::Win32::UI::Shell::SHFMT_OPT_FULL,
        )
    };
    format_result(result)
}

fn format_drive_letter(drive: &str) -> Result<char> {
    let bytes = drive.as_bytes();
    anyhow::ensure!(
        matches!(bytes, [letter] if letter.is_ascii_alphabetic())
            || matches!(bytes, [letter, b':'] if letter.is_ascii_alphabetic())
            || matches!(bytes, [letter, b':', b'\\' | b'/'] if letter.is_ascii_alphabetic()),
        "格式化需要准确的本地驱动器根路径：{drive}"
    );
    Ok((bytes[0] as char).to_ascii_uppercase())
}

fn format_result(result: u32) -> Result<()> {
    match SHFMT_RET(result) {
        SHFMT_ERROR => bail!("Windows 格式化对话框返回错误 (SHFMT_ERROR)"),
        SHFMT_NOFORMAT => bail!("Windows 无法格式化此驱动器 (SHFMT_NOFORMAT)"),
        // Closing/cancelling the dialog is not a successful format or an error.
        SHFMT_CANCEL => Ok(()),
        _ => Ok(()),
    }
}

/// Launch the Windows-owned formatting workflow, without interpreting the
/// elevated dialog host's eventual process exit code as a file operation result.
fn elevate_self(arguments: &[&str]) -> Result<()> {
    let file = wide(std::env::current_exe()?.as_os_str());
    let parameters = wide(arguments.join(" ").as_str());
    let verb = wide("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.context("无法以管理员身份启动该操作")?;
    if !info.hProcess.is_invalid() {
        // This request opens a system dialog; it does not perform a format.
        // Cancellation and closing the external host are handled by Windows.
        // Waiting here could leave the application busy after its dialog closes,
        // and an external host exit code is not a reliable formatting outcome.
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

fn invoke_verb(paths: &[PathBuf], verb: &str) -> Result<()> {
    // "Format" is not a registry verb: it comes from the Shell's drive menu and
    // needs the elevated format UI, so relaunch ourselves with `runas`.
    if verb.eq_ignore_ascii_case("format") {
        anyhow::ensure!(paths.len() == 1, "请仅选择一个要格式化的驱动器");
        let letter = format_drive_letter(&paths[0].to_string_lossy())?;
        elevate_self(&["--format-drive", &letter.to_string()])?;
        return Ok(());
    }
    if is_start_pin_verb(verb) {
        anyhow::ensure!(paths.len() == 1, "请仅选择一个项目");
        return invoke_start_pin(&paths[0], verb, HWND::default());
    }
    for path in paths {
        let file = wide(path.as_os_str());
        let action = wide(verb);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            // NO_UI suppresses the Shell's own error dialogs (which would block this
            // helper); the verb's own dialog, e.g. the format dialog, still opens.
            fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(action.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }.context("无法执行该操作")?;
    }
    unsafe {
        pump_property_windows();
    }
    Ok(())
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
/// Inspect already populated submenu handles only. No WM_INITMENUPOPUP here:
/// selecting an unrelated command must not cold-load every Shell extension.
fn selected_menu_label(popup: HMENU, command: u32, depth: u8) -> Option<String> {
    if depth > 5 {
        return None;
    }
    unsafe {
        for index in 0..GetMenuItemCount(Some(popup)).clamp(0, 256) {
            let mut text = [0u16; 1024];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID | MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
                cch: text.len() as u32,
                ..Default::default()
            };
            if GetMenuItemInfoW(popup, index as u32, true, &mut item).is_err() {
                continue;
            }
            if item.wID == command {
                return Some(String::from_utf16_lossy(
                    &text[..text.iter().position(|c| *c == 0).unwrap_or(text.len())],
                ));
            }
            if !item.hSubMenu.is_invalid() {
                if let Some(label) = selected_menu_label(item.hSubMenu, command, depth + 1) {
                    return Some(label);
                }
            }
        }
    }
    None
}
/// Shell's background Sharing submenu expects Explorer's IShellView site.
/// Keep the system's parent label, but give its context-appropriate commands application
/// routing. No extension submenu is initialized just to construct this menu.
unsafe fn bridge_sharing_submenu(
    popup: HMENU,
    path: &Path,
    commands: &mut Vec<String>,
) -> Result<Option<Popup>> {
    let title = sharing_native::parent_title(path)?;
    unsafe {
        for position in 0..GetMenuItemCount(Some(popup)).clamp(0, 256) {
            let mut text = [0u16; 1024];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
                cch: text.len() as u32,
                ..Default::default()
            };
            if GetMenuItemInfoW(popup, position as u32, true, &mut item).is_err()
                || item.hSubMenu.is_invalid()
            {
                continue;
            }
            let label = String::from_utf16_lossy(
                &text[..text.iter().position(|c| *c == 0).unwrap_or(text.len())],
            );
            if normalized_menu_title(&label) != normalized_menu_title(&title) {
                continue;
            }
            let titles = if sharing_native::is_root(path) {
                vec![(crate::i18n::text("高级共享…").to_owned(), "advanced-share")]
            } else {
                vec![
                    (sharing_native::title(path, true)?, "remove-access"),
                    (sharing_native::title(path, false)?, "grant-access"),
                ]
            };
            let replacement = CreatePopupMenu()?;
            for (text, action) in titles {
                let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                let id = 0x9000 + commands.len();
                if let Err(error) = AppendMenuW(replacement, MF_STRING, id, PCWSTR(text.as_ptr())) {
                    let _ = DestroyMenu(replacement);
                    return Err(error.into());
                }
                commands.push(action.into());
            }
            let old = item.hSubMenu;
            item.fMask = MIIM_SUBMENU;
            item.hSubMenu = replacement;
            if let Err(error) = SetMenuItemInfoW(popup, position as u32, true, &item) {
                let _ = DestroyMenu(replacement);
                return Err(error.into());
            }
            // The root owns the replacement; retain the detached old handle only
            // until this menu closes, then release it on the same STA.
            return Ok(Some(Popup(old)));
        }
    }
    Ok(None)
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
        // Only initialize the installed Sharing submenu. Enumerating every
        // extension's submenu here can block on unrelated providers.
        let parent_title = sharing_native::parent_title(path).unwrap_or_default();
        let mut entries = Vec::new();
        for index in 0..GetMenuItemCount(Some(popup.0)).clamp(0, 256) {
            let mut text = [0u16; 1024];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_ID | MIIM_STRING | MIIM_SUBMENU,
                dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
                cch: text.len() as u32,
                ..Default::default()
            };
            if GetMenuItemInfoW(popup.0, index as u32, true, &mut item).is_err()
                || item.hSubMenu.is_invalid()
            {
                continue;
            }
            let label = String::from_utf16_lossy(
                &text[..text.iter().position(|n| *n == 0).unwrap_or(text.len())],
            );
            let verb = canonical_verb(&menu, item.wID);
            if verb == "windows.share"
                || (!parent_title.is_empty()
                    && normalized_menu_title(&label) == normalized_menu_title(&parent_title))
            {
                if let Ok(extension) = menu.cast::<IContextMenu3>() {
                    let _ = extension.HandleMenuMsg2(
                        WM_INITMENUPOPUP,
                        WPARAM(item.hSubMenu.0 as usize),
                        LPARAM(index as isize),
                        None,
                    );
                } else if let Ok(extension) = menu.cast::<IContextMenu2>() {
                    let _ = extension.HandleMenuMsg(
                        WM_INITMENUPOPUP,
                        WPARAM(item.hSubMenu.0 as usize),
                        LPARAM(index as isize),
                    );
                }
                entries = menu_entries(&menu, item.hSubMenu, 1);
                break;
            }
        }
        fn find<'a>(entries: &'a [MenuEntry], title: &str) -> Option<&'a MenuEntry> {
            entries.iter().find_map(|entry| {
                if normalized_menu_title(&entry.label) == title {
                    Some(entry)
                } else {
                    find(&entry.children, title)
                }
            })
        }
        // Canonical parent verbs vary across Windows versions; match the title
        // supplied by the installed command, including cascaded submenu entries.
        let target = find(&entries, &title);
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
    let output = context_menu_options(
        paths,
        folder,
        x,
        y,
        Some(crate::theme::Theme::system().dark),
        false,
        false,
        false,
        None,
        false,
        None,
    )?;
    print!("{output}");
    Ok(())
}

/// Convert a serialized window handle carried by a request into an `HWND`.
fn owner_hwnd(owner: u64) -> Option<HWND> {
    (owner != 0).then_some(HWND(owner as usize as *mut _))
}

/// HWND of the calling thread's active window. Passing it in a menu request makes
/// the menu run inside the application process, owned by that window, so the app
/// stays the active process (no helper steals activation).
pub fn active_window_owner() -> u64 {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow().0 as usize as u64 }
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
fn is_start_pin_verb(verb: &str) -> bool {
    matches!(
        verb,
        "pintostartscreen" | "unpinfromstartscreen" | "startpin" | "startunpin"
    )
}

/// Per-menu cache for `GetCommandString`. A rich context menu queries the same
/// command ids three times (insert position, host-verb filtering, selected verb);
/// every miss is a cross-module call into a third-party handler.
struct VerbCache<'a> {
    menu: &'a IContextMenu,
    cache: std::collections::HashMap<u32, String>,
}
impl<'a> VerbCache<'a> {
    fn new(menu: &'a IContextMenu) -> Self {
        Self {
            menu,
            cache: std::collections::HashMap::new(),
        }
    }
    fn verb(&mut self, command: u32) -> String {
        if let Some(value) = self.cache.get(&command) {
            return value.clone();
        }
        let value = canonical_verb(self.menu, command);
        self.cache.insert(command, value.clone());
        value
    }
}

fn start_pin_failure(error: windows::core::Error) -> anyhow::Error {
    if error.code() == windows::core::HRESULT(0x80070005u32 as i32) {
        anyhow::anyhow!(
            "Windows 拒绝执行“固定到开始/从开始取消固定”（0x80070005）。这不一定是文件访问权限不足，也可能受到开始菜单策略或系统 Shell 限制。请在 Windows 资源管理器中对该项目执行此操作。未修改权限或尝试管理员重试。系统信息：{error}"
        )
    } else {
        anyhow::Error::new(error).context("Windows 无法执行开始菜单固定操作")
    }
}

/// Let ShellExecute bind the item's own IContextMenu handler. NOASYNC is
/// intentional: this isolated menu host can exit once the invocation finishes.
/// ASYNCOK would require a longer-lived thread reference/message loop.
fn invoke_start_pin(path: &Path, verb: &str, owner: HWND) -> Result<()> {
    anyhow::ensure!(is_start_pin_verb(verb), "Invalid Start pin verb");
    let file = wide(path);
    let verb = wide(verb);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: owner,
        lpFile: PCWSTR(file.as_ptr()),
        lpVerb: PCWSTR(verb.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(start_pin_failure)?;
    unsafe {
        pump_property_windows();
    }
    Ok(())
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
    mut options: Option<native_menu::Options>,
    quick_access_pinned: bool,
    app_owner: Option<HWND>,
) -> Result<String> {
    let mut output = String::new();
    macro_rules! emit {
        ($($arg:tt)*) => {{
            output.push_str(&format!($($arg)*));
            output.push('\n');
        }};
    }
    menu_trace(format!(
        "menu: begin paths={} folder={}",
        paths.len(),
        folder.display()
    ));
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
        // A 1x1 popup with nothing painted is already invisible. Do NOT make it
        // layered: a freshly created layered window that the compositor has not
        // shown yet makes the Shell cancel the first menu it owns (the retry used
        // to succeed only because the window had been composed by then).
        // In-process the owner is a *child* of the application window: child windows
        // share the main window's activation, so the main window keeps its DWM
        // active frame (shadow) while the menu is open. A separate top-level popup
        // took activation and made the frame flicker.
        let in_process = app_owner.is_some();
        let hwnd = CreateWindowExW(
            if in_process {
                WINDOW_EX_STYLE(0)
            } else {
                WS_EX_TOOLWINDOW
            },
            class,
            w!("Rust Explorer"),
            if in_process {
                WS_CHILD | WS_VISIBLE
            } else {
                WS_POPUP | if inspect { WINDOW_STYLE(0) } else { WS_VISIBLE }
            },
            x,
            y,
            1,
            1,
            // Owned by the application window when the menu runs in-process.
            app_owner,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?;
        let _owner = MenuWindow(hwnd);
        menu_trace("menu: owner window ready");
        menu_trace("menu: owner window ready");
        let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
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
            let pidl = menu_pidl(folder)?;
            if pidl.0.is_null() {
                bail!("无法读取文件夹菜单");
            }
            let desktop: IShellFolder = SHGetDesktopFolder()?;
            let folder: IShellFolder = desktop.BindToObject(pidl.0, None)?;
            folder.CreateViewObject(hwnd)?
        } else {
            let mut items = Vec::new();
            for path in paths {
                let pidl = menu_pidl(path)?;
                if pidl.0.is_null() {
                    bail!("无法读取文件菜单");
                }
                items.push(pidl);
            }
            let parent: IShellFolder = SHBindToParent(items[0].0, None)?;
            // Cross-folder selections are exposed through IShellItemArray's data object.
            let first_parent = paths[0]
                .parent()
                .map(|parent| parent.to_string_lossy().to_lowercase());
            let same_parent = paths.iter().all(|p| {
                p.parent()
                    .map(|parent| parent.to_string_lossy().to_lowercase())
                    == first_parent
            });
            if !same_parent {
                bail!("跨目录多选菜单请先打开所在位置；复制/移动/删除仍支持跨目录多选");
            }
            let children: Vec<*const Common::ITEMIDLIST> = items
                .iter()
                .map(|v| ILFindLastID(v.0) as *const _)
                .collect();
            parent.GetUIObjectOf(hwnd, &children, None)?
        };
        menu_trace("menu: IContextMenu ready");
        let popup = Popup(CreatePopupMenu()?);
        menu.QueryContextMenu(
            popup.0,
            0,
            1,
            0x6fff,
            CMF_NORMAL | CMF_CANRENAME | CMF_EXPLORE,
        )
        .ok()?;
        menu_trace(format!(
            "menu: QueryContextMenu done, {} items",
            GetMenuItemCount(Some(popup.0))
        ));
        MENU.with(|v| *v.borrow_mut() = Some(menu.clone()));
        let mut verbs = VerbCache::new(&menu);
        if let Some(options) = &mut options {
            options.folder = !paths.is_empty() && paths.iter().any(|path| is_folder(path));
            options.folder_insert = if options.folder {
                (0..GetMenuItemCount(Some(popup.0)))
                    .find(|position| verbs.verb(GetMenuItemID(popup.0, *position)) == "open")
                    .map(|position| position as u32 + 1)
                    .unwrap_or(0)
            } else {
                0
            };
        }
        let mut commands = if let Some(options) = &options {
            // Shell view commands need our own IShellView state; keep all other
            // provider commands, including the real New and sharing submenus.
            for position in (0..GetMenuItemCount(Some(popup.0))).rev() {
                let verb = verbs.verb(GetMenuItemID(popup.0, position));
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
        let sharing_path = if paths.len() == 1 { &paths[0] } else { folder };
        let _detached_sharing =
            if app_owner.is_some() && !computer && paths.len() <= 1 && sharing_path.is_dir() {
                // Failure to find/install this optional bridge leaves the original
                // menu intact; owner/STA/message-pump lifecycle remains unchanged.
                bridge_sharing_submenu(popup.0, sharing_path, &mut commands)
                    .ok()
                    .flatten()
            } else {
                None
            };
        if quick_access_pinned {
            // The file-system folder menu always exposes `pintohome`, even for a
            // folder that is already in Quick Access. Present the truthful action
            // instead; the selection is intercepted below and routed to the
            // canonical remove/unpin verb.
            for position in 0..GetMenuItemCount(Some(popup.0)) {
                let id = GetMenuItemID(popup.0, position);
                if verbs.verb(id) == "pintohome" {
                    let text = wide("从快速访问中移除");
                    let _ = ModifyMenuW(
                        popup.0,
                        position as u32,
                        MF_BYPOSITION | MF_STRING,
                        id as usize,
                        PCWSTR(text.as_ptr()),
                    );
                    break;
                }
            }
        }
        if inspect {
            emit!(
                "{}",
                serde_json::to_string(&menu_entries(&menu, popup.0, 0))?
            );
            return Ok(output);
        }
        if computer {
            let native_quick_pin = (0..GetMenuItemCount(Some(popup.0)))
                .any(|index| verbs.verb(GetMenuItemID(popup.0, index)) == "pintohome");
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
                    crate::i18n::wide("固定到快速访问"),
                )?;
            }
            // Guarantee a rename entry: the window owns the name and shows its
            // inline editor on the "This PC" label, whatever the Shell offers.
            let native_rename = (0..GetMenuItemCount(Some(popup.0)))
                .any(|index| verbs.verb(GetMenuItemID(popup.0, index)) == "rename");
            if !native_rename {
                InsertMenuW(
                    popup.0,
                    3,
                    MF_BYPOSITION | MF_STRING,
                    0x8005,
                    crate::i18n::wide("重命名(&M)"),
                )?;
            }
            InsertMenuW(
                popup.0,
                if native_quick_pin { 3 } else { 4 },
                MF_BYPOSITION | MF_SEPARATOR,
                0,
                PCWSTR::null(),
            )?;
            AppendMenuW(
                popup.0,
                MF_STRING,
                0x8004,
                crate::i18n::wide("添加网络位置(&L)…"),
            )?;
        }
        if !wait_for_context_gesture(hwnd, x, y) {
            return Ok(output);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Only a helper process has to fight for the foreground. In-process the
        // application is already active, and claiming it would drop the main
        // window's active frame for as long as the menu is open.
        if app_owner.is_none() {
            claim_menu_foreground(hwnd);
        }
        menu_trace(format!(
            "host: showing menu at {x},{y} (foreground={} active={} rb_down={})",
            GetForegroundWindow() == hwnd,
            windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow() == hwnd,
            (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                windows::Win32::UI::Input::KeyboardAndMouse::VK_RBUTTON.0 as i32
            ) as u16
                & 0x8000)
                != 0
        ));
        // One shot: the gesture is already debounced (wait for the release, 12 px of
        // press-to-release travel) and the menu is shown in-process, so it no longer
        // needs the old "re-show when it vanishes too fast" retry loop.
        // Shell extensions can alter the process menu theme while constructing
        // a file menu. Reapply after QueryContextMenu, immediately before showing
        // it, just as for folder/background menus. Preserve the same owner/STA.
        if let Some(dark) = dark {
            native_menu::configure_theme(hwnd, dark);
        }
        let started = std::time::Instant::now();
        let command = TrackPopupMenuEx(
            popup.0,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            x,
            y,
            hwnd,
            None,
        )
        .0;
        menu_trace(format!(
            "host: menu returned command {command} after {} ms (GetLastError={:?})",
            started.elapsed().as_millis(),
            GetLastError()
        ));
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        if let Some(action) = (command as usize)
            .checked_sub(0x9000)
            .and_then(|index| commands.get(index))
        {
            emit!("command:{action}");
            return Ok(output);
        }
        if computer {
            if command == 0x8004 {
                add_network_location()?;
                return Ok(output);
            }
            let action = match command {
                0x8000 => Some("computer-expand"),
                0x8001 => Some("computer-tab"),
                0x8002 => Some("computer-window"),
                0x8003 => Some("computer-pin"),
                0x8005 => Some("computer-rename"),
                _ => None,
            };
            if let Some(action) = action {
                emit!("command:{action}");
                return Ok(output);
            }
        }
        if command > 0 {
            let verb = verbs.verb(command as u32);
            // Sharing submenu children do not consistently provide canonical
            // verbs. Resolve only the selected item's existing label, after the
            // popup closes; never eagerly load submenus before displaying it.
            let sharing_path = if paths.len() == 1 { &paths[0] } else { folder };
            if app_owner.is_some()
                && paths.len() <= 1
                && (verb.is_empty() || verb.contains("share"))
                && sharing_path.is_dir()
            {
                if let Some(label) = selected_menu_label(popup.0, command as u32, 0) {
                    for (remove, action) in [(false, "grant-access"), (true, "remove-access")] {
                        if sharing_native::title(sharing_path, remove).is_ok_and(|title| {
                            normalized_menu_title(&title) == normalized_menu_title(&label)
                        }) {
                            emit!("command:{action}");
                            return Ok(output);
                        }
                    }
                }
            }
            menu_trace(format!(
                "menu: chosen verb {verb:?} command={command} paths={}",
                paths.len()
            ));
            if verb == "pintohome" && !computer && paths.len() == 1 {
                // Route the Quick Access toggle through the canonical verbs so the
                // pinned state always matches the label the menu showed.
                if crate::quick_access::set_pinned(&paths[0], !quick_access_pinned).is_ok() {
                    emit!(
                        "command:{}",
                        if quick_access_pinned {
                            "quick-unpinned"
                        } else {
                            "quick-pinned"
                        }
                    );
                    return Ok(output);
                }
            }
            if computer && verb == "pintohome" {
                emit!("command:computer-pin");
                return Ok(output);
            }
            if computer && verb == "rename" {
                // The window owns the name: it shows an inline editor at the
                // "This PC" label and sends the resulting name back.
                emit!("command:computer-rename");
                return Ok(output);
            }
            if computer && matches!(verb.as_str(), "properties" | "open" | "explore") {
                emit!(
                    "command:{}",
                    match verb.as_str() {
                        "properties" => "computer-properties",
                        _ => "computer-open",
                    }
                );
                return Ok(output);
            }
            if verb == "rename" {
                emit!("rename");
                return Ok(output);
            }
            if verb == "properties" {
                emit!("properties");
                return Ok(output);
            }
            if paths.is_empty() && verb == "newfolder" {
                // Shell's New handler expects an Explorer IShellView for rename
                // activation. Let our existing asynchronous create/select/rename
                // workflow own this operation instead.
                emit!("command:new-folder");
                return Ok(output);
            }
            if matches!(verb.as_str(), "open" | "explore") && !paths.is_empty() {
                emit!("command:file-open");
                return Ok(output);
            }
            // The Shell's own "open in new window / tab" verbs launch a brand-new
            // process, which cannot inherit this window's view settings. Route them
            // through the application so the new window matches the current one.
            if let Some(kind) = verb.strip_prefix("opennew") {
                menu_trace(format!("menu: verb {verb} routed as a new window request"));
                let _ = kind;
                emit!(
                    "command:{}",
                    if verb.contains("tab") {
                        "open-new-tab"
                    } else {
                        "open-new-window"
                    }
                );
                return Ok(output);
            }
            // The Shell copy/cut verbs can fail or leave an empty clipboard when
            // invoked from this short-lived menu host. Write CF_HDROP ourselves
            // so a traditional-menu Copy always lands on the Windows clipboard.
            if matches!(verb.as_str(), "copy" | "cut") && !paths.is_empty() {
                crate::shell::copy_to_clipboard(paths, verb == "cut")?;
                return Ok(output);
            }
            if is_start_pin_verb(&verb)
                || matches!(verb.as_str(), "format" | "print" | "share" | "burn")
            {
                // Return the request to the UI so failures are displayed, instead
                // of discarding them in an unobserved detached thread.
                let request = Request::InvokeVerb {
                    paths: if paths.is_empty() {
                        vec![folder.to_path_buf()]
                    } else {
                        paths.to_vec()
                    },
                    verb: verb.clone(),
                };
                emit!("command:shell-verb:{}", serde_json::to_string(&request)?);
                return Ok(output);
            }
            if is_start_pin_verb(&verb) && paths.len() == 1 {
                invoke_start_pin(&paths[0], &verb, hwnd)?;
                return Ok(output);
            }
            // `delete` and `paste` show a confirmation/progress dialog and, with
            // SEE_MASK_NOASYNC, block the calling thread until they finish. Now that
            // the menu runs inside the application, blocking here would freeze the
            // menu task forever, so route them to the application's own paths, which
            // run in an isolated helper and keep the UI responsive.
            if matches!(verb.as_str(), "delete" | "paste") {
                emit!("command:file-{verb}");
                return Ok(output);
            }
            // Verbs that open a modal dialog of their own (format, print, share,
            // burn) block the calling thread until the dialog closes. Delegate them
            // to a detached helper so the application never waits for them.
            let mut info = CMINVOKECOMMANDINFOEX {
                cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                // `SEE_MASK_NOASYNC` forces the verb to finish before InvokeCommand
                // returns. That is required in a short-lived helper, but in-process
                // it blocks the menu task whenever the Shell shows a modal dialog
                // (delete, paste, format, print, share, ...). Inside the application
                // the Shell may run the verb asynchronously, so dialogs open on their
                // own thread and the UI stays responsive.
                fMask: if app_owner.is_some() {
                    SEE_MASK_UNICODE
                } else {
                    SEE_MASK_UNICODE | SEE_MASK_NOASYNC
                },
                hwnd,
                lpVerb: PCSTR((command as usize - 1) as *const u8),
                lpVerbW: PCWSTR((command as usize - 1) as *const u16),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let invoked = menu.InvokeCommand(&mut info as *mut _ as *const CMINVOKECOMMANDINFO);
            if is_start_pin_verb(&verb) {
                invoked.map_err(start_pin_failure)?;
            } else {
                invoked?;
            }
            if verb == "delete" {
                // The helper waits for the native operation before the UI
                // requests its next directory snapshot.
                emit!("refresh");
            }
            if verb == "pintohome" {
                // Multi-selection pins (and any fallback) happen inside the Shell;
                // ask the UI to pull the new Quick Access state immediately.
                emit!("command:quick-pinned");
            }
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
                pump_property_windows_scoped(app_owner.is_some());
            }
        }
        Ok(output)
    }
}
