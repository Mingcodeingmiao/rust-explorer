//! Windows OLE drag/drop. COM interfaces never cross apartment boundaries.
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
};

unsafe extern "C" {
    fn re_register_file_drop(
        owner: isize,
        callback: extern "C" fn(i32, f32, f32, *mut u16, u32, *mut c_void) -> i32,
        context: *mut c_void,
        registration: *mut *mut c_void,
    ) -> i32;
    fn re_unregister_file_drop(owner: isize, registration: *mut c_void);
    fn re_queue_file_drag(registration: *mut c_void, paths: *const *const u16, count: u32) -> i32;
}

pub fn trace(message: &str) {
    if let Some(path) = std::env::var_os("RUST_EXPLORER_DRAG_TRACE") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}

struct Context {
    resolve: Box<dyn Fn(f32, f32) -> Option<PathBuf>>,
    completed: Cell<bool>,
    enabled: Cell<bool>,
    source_tx: mpsc::Sender<Result<u32, String>>,
}
extern "C" fn callback(
    kind: i32,
    x: f32,
    y: f32,
    buffer: *mut u16,
    capacity: u32,
    context: *mut c_void,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if kind == 2 {
            unsafe {
                drop(Box::from_raw(context as *mut Context));
            }
            return 1;
        }
        let context = unsafe { &*(context as *const Context) };
        if kind == 3 || kind == 4 {
            let result = if kind == 3 {
                Ok(x as u32)
            } else {
                let text = unsafe { std::slice::from_raw_parts(buffer, capacity as usize) };
                let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
                Err(String::from_utf16_lossy(&text[..end]))
            };
            let _ = context.source_tx.send(result);
            return 1;
        }
        if kind == 1 {
            context.completed.set(true);
            return 1;
        }
        if !context.enabled.get() {
            return 0;
        }
        let Some(path) = (context.resolve)(x, y) else {
            return 0;
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if buffer.is_null() || wide.len() > capacity as usize {
            return 0;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), buffer, wide.len());
        }
        1
    }))
    .unwrap_or(0)
}

/// Own this registration on the window's UI STA, including destruction.
pub struct DropTarget {
    owner: isize,
    registration: *mut c_void,
    context: *mut Context,
    pub background: Rc<RefCell<Option<PathBuf>>>,
    source_rx: mpsc::Receiver<Result<u32, String>>,
}
impl DropTarget {
    pub fn register(
        owner: isize,
        background: Rc<RefCell<Option<PathBuf>>>,
        resolve: impl Fn(f32, f32) -> Option<PathBuf> + 'static,
    ) -> anyhow::Result<Self> {
        let (source_tx, source_rx) = mpsc::channel();
        let context = Box::into_raw(Box::new(Context {
            resolve: Box::new(resolve),
            completed: Cell::new(false),
            enabled: Cell::new(true),
            source_tx,
        }));
        let mut registration = std::ptr::null_mut();
        let hr =
            unsafe { re_register_file_drop(owner, callback, context.cast(), &mut registration) };
        anyhow::ensure!(hr >= 0, "RegisterDragDrop failed: 0x{:08X}", hr as u32);
        Ok(Self {
            owner,
            registration,
            context,
            background,
            source_rx,
        })
    }
    pub fn take_completed(&self) -> bool {
        unsafe { (*self.context).completed.replace(false) }
    }
    pub fn set_enabled(&self, enabled: bool) {
        unsafe {
            (*self.context).enabled.set(enabled);
        }
    }
    pub fn source_result(&self) -> Option<Result<u32, String>> {
        self.source_rx.try_recv().ok()
    }
    pub fn start(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        anyhow::ensure!(!paths.is_empty(), "No files selected for dragging");
        let wide: Vec<Vec<u16>> = paths
            .iter()
            .map(|path| path.as_os_str().encode_wide().chain(Some(0)).collect())
            .collect();
        let pointers: Vec<*const u16> = wide.iter().map(|path| path.as_ptr()).collect();
        let hr = unsafe {
            re_queue_file_drag(self.registration, pointers.as_ptr(), pointers.len() as u32)
        };
        anyhow::ensure!(
            hr >= 0,
            "Cannot start Windows drag/drop: 0x{:08X}",
            hr as u32
        );
        Ok(())
    }
}
impl Drop for DropTarget {
    fn drop(&mut self) {
        unsafe {
            re_unregister_file_drop(self.owner, self.registration);
        }
    }
}
