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
        drop: extern "C" fn(*const *const u16, u32, *const u16, u32, *mut c_void),
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

/// Latest position/target observed by the OLE drop target. OLE owns the mouse
/// during a left-button drag, so GPUI receives no reliable mouse-move stream;
/// the target callback updates this state and the renderer paints the same
/// operation label the right-drag path uses.
#[derive(Clone, Debug, Default)]
pub struct DragFeedback {
    /// HWND of the window whose drop target produced this feedback. Two
    /// application windows share the UI thread, so the consumer must ignore
    /// feedback that belongs to another window.
    pub window: isize,
    pub position: (f32, f32),
    pub target: Option<PathBuf>,
    /// Number of dragged items, read from CF_HDROP by the target. It lets a
    /// window that is not the drag source render the same operation label.
    pub count: u32,
    /// Current OLE drop effect (COPY=1, MOVE=2, LINK=4).
    pub effect: u32,
}
thread_local! {
    static FEEDBACK: RefCell<Option<DragFeedback>> = const { RefCell::new(None) };
    static COUNT: Cell<u32> = const { Cell::new(0) };
    static EFFECT: Cell<u32> = const { Cell::new(0) };
}
pub fn set_drag_count(count: u32) {
    COUNT.with(|slot| slot.set(count));
}
pub fn set_drag_effect(effect: u32) {
    EFFECT.with(|slot| slot.set(effect));
}
pub fn set_feedback(window: isize, x: f32, y: f32, target: Option<PathBuf>) {
    let count = COUNT.with(Cell::get);
    let effect = EFFECT.with(Cell::get);
    FEEDBACK.with(|slot| {
        *slot.borrow_mut() = Some(DragFeedback {
            window,
            position: (x, y),
            target,
            count,
            effect,
        });
    });
}
pub fn feedback() -> Option<DragFeedback> {
    FEEDBACK.with(|slot| slot.borrow().clone())
}
pub fn clear_feedback() {
    FEEDBACK.with(|slot| *slot.borrow_mut() = None);
    COUNT.with(|slot| slot.set(0));
    EFFECT.with(|slot| slot.set(0));
}

/// One completed native drop. The transfer itself runs in the cancellable
/// helper, never on the UI thread that is still inside OLE's Drop callback.
pub struct DropRequest {
    pub paths: Vec<PathBuf>,
    pub destination: PathBuf,
    pub effect: u32,
}

struct Context {
    resolve: Box<dyn Fn(f32, f32) -> Option<PathBuf>>,
    completed: Cell<bool>,
    enabled: Cell<bool>,
    source_tx: mpsc::Sender<Result<u32, String>>,
    drop_tx: mpsc::Sender<DropRequest>,
}
fn wide_text(mut pointer: *const u16) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let start = pointer;
    let mut length = 0usize;
    while length < 32768 {
        if unsafe { *pointer } == 0 {
            let text = unsafe { std::slice::from_raw_parts(start, length) };
            return Some(String::from_utf16_lossy(text));
        }
        pointer = unsafe { pointer.add(1) };
        length += 1;
    }
    None
}
extern "C" fn drop_callback(
    paths: *const *const u16,
    count: u32,
    destination: *const u16,
    effect: u32,
    context: *mut c_void,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if paths.is_null() || destination.is_null() || context.is_null() || count == 0 {
            return;
        }
        let context = unsafe { &*(context as *const Context) };
        let mut collected = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            let pointer = unsafe { *paths.add(index) };
            let Some(text) = wide_text(pointer) else {
                return;
            };
            collected.push(PathBuf::from(text));
        }
        let Some(destination) = wide_text(destination) else {
            return;
        };
        let _ = context.drop_tx.send(DropRequest {
            paths: collected,
            destination: PathBuf::from(destination),
            effect,
        });
    }));
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
        if kind == 6 {
            set_drag_count(x.max(0.0) as u32);
            return 1;
        }
        if kind == 7 {
            set_drag_effect(x.max(0.0) as u32);
            return 1;
        }
        if kind == 2 {
            clear_feedback();
            unsafe {
                drop(Box::from_raw(context as *mut Context));
            }
            return 1;
        }
        if kind == 5 {
            // Pointer left every registered drop region: no next DragOver will
            // arrive to clear the operation label.
            clear_feedback();
            return 1;
        }
        let context = unsafe { &*(context as *const Context) };
        if kind == 3 || kind == 4 {
            clear_feedback();
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
            clear_feedback();
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
    drop_rx: mpsc::Receiver<DropRequest>,
}
impl DropTarget {
    pub fn register(
        owner: isize,
        background: Rc<RefCell<Option<PathBuf>>>,
        resolve: impl Fn(f32, f32) -> Option<PathBuf> + 'static,
    ) -> anyhow::Result<Self> {
        let (source_tx, source_rx) = mpsc::channel();
        let (drop_tx, drop_rx) = mpsc::channel();
        let context = Box::into_raw(Box::new(Context {
            resolve: Box::new(resolve),
            completed: Cell::new(false),
            enabled: Cell::new(true),
            source_tx,
            drop_tx,
        }));
        let mut registration = std::ptr::null_mut();
        let hr = unsafe {
            re_register_file_drop(
                owner,
                callback,
                drop_callback,
                context.cast(),
                &mut registration,
            )
        };
        anyhow::ensure!(hr >= 0, "RegisterDragDrop failed: 0x{:08X}", hr as u32);
        Ok(Self {
            owner,
            registration,
            context,
            background,
            source_rx,
            drop_rx,
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
    pub fn take_drop(&self) -> Option<DropRequest> {
        self.drop_rx.try_recv().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_callback_copies_wide_paths_and_effect() {
        let (source_tx, _source_rx) = mpsc::channel();
        let (drop_tx, drop_rx) = mpsc::channel();
        let context = Box::into_raw(Box::new(Context {
            resolve: Box::new(|_, _| None),
            completed: Cell::new(false),
            enabled: Cell::new(true),
            source_tx,
            drop_tx,
        }));
        let source = "C:\\opt\\mingw64"
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        assert_eq!(
            wide_text(source.as_ptr()),
            Some("C:\\opt\\mingw64".to_string())
        );
        let destination = "R:\\".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let pointers = [source.as_ptr()];
        drop_callback(
            pointers.as_ptr(),
            1,
            destination.as_ptr(),
            1,
            context.cast(),
        );
        let request = drop_rx.try_recv().expect("drop request");
        assert_eq!(request.paths, vec![PathBuf::from("C:\\opt\\mingw64")]);
        assert_eq!(request.destination, PathBuf::from("R:\\"));
        assert_eq!(request.effect, 1);
        unsafe { drop(Box::from_raw(context)) };
    }

    #[test]
    fn drag_feedback_round_trips_and_clears() {
        clear_feedback();
        assert!(feedback().is_none());
        set_drag_count(3);
        set_drag_effect(2);
        set_feedback(42, 12.5, 34.5, Some(PathBuf::from(r"C:\target")));
        let value = feedback().expect("feedback");
        assert_eq!(value.window, 42);
        assert_eq!(value.position, (12.5, 34.5));
        assert_eq!(value.target, Some(PathBuf::from(r"C:\target")));
        assert_eq!(value.count, 3);
        assert_eq!(value.effect, 2);
        set_feedback(7, 1.0, 2.0, None);
        let value = feedback().expect("feedback");
        assert_eq!(value.window, 7);
        assert_eq!(value.target, None);
        clear_feedback();
        assert!(feedback().is_none());
    }
}
