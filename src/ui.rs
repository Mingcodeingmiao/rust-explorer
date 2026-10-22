use crate::input::{self, InputEvent, TextInput};
use crate::visual::{self, ui};
use gpui::{prelude::*, *};
use notify::Watcher;
use rust_explorer::{
    disk_usage::{self, Usage},
    list_layout::{self, ListCell},
    new_file,
    settings::{Settings, ViewMode},
    volume_label,
    volumes::{self, Volume},
};
use rust_explorer::{
    fs::{self, Entry, History, Sort},
    native_assets::{self, PreviewKind},
    preview::{self, Kind},
    search, shell,
    theme::Theme,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use std::{collections::HashMap, time::Instant};
#[path = "ui_extras.rs"]
mod extras;
#[path = "ui_navigation.rs"]
mod navigation;
use extras::*;

actions!(
    explorer,
    [
        Back,
        Forward,
        Up,
        Refresh,
        Address,
        Search,
        Open,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Delete,
        PermanentDelete,
        Rename,
        NewFolder,
        SelectAll,
        Next,
        Previous,
        TogglePreview,
        NewTab,
        CloseTab,
        Escape,
        ContextMenu,
        Properties,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        NewWindow,
        NextTab,
        PreviousTab,
        ToggleFocused,
        SelectFocused,
        ToggleDetails,
        CycleFocus,
        CycleFocusBack,
        FullScreen,
        ExpandCurrentTree
    ]
);

#[derive(Clone, Debug, PartialEq)]
enum NavigationStep {
    Horizontal(isize),
    Vertical(isize),
    Page(isize),
    First,
    Last,
}
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = explorer, no_json)]
struct NavigateFiles {
    step: NavigationStep,
    extend: bool,
    focus_only: bool,
}
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = explorer, no_json)]
struct ActivateNumberedTab {
    index: usize,
}
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = explorer, no_json)]
struct ChangeNumberedView {
    index: usize,
}
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = explorer, no_json)]
struct NavigateTree {
    command: rust_explorer::nav_keyboard::Command,
}

#[derive(Clone)]
struct Tab {
    id: u64,
    history: History,
    query: String,
    global: bool,
    reveal_targets: Vec<PathBuf>,
}
#[derive(Clone)]
struct DragFiles {
    paths: Vec<PathBuf>,
}
#[derive(Clone)]
struct FileDragState {
    paths: Vec<PathBuf>,
    position: Point<Pixels>,
    target: Option<PathBuf>,
    mode: shell::TransferMode,
}
/// Pointer travel that turns a right-click into a right-drag. The system drag
/// rectangle (`SM_CXDRAG`) is about 4 px, which a normal hand jitter can exceed
/// while still intending a click, so give the gesture noticeably more slack; the
/// native menu host applies the same tolerance before it refuses to show a menu.
const RIGHT_DRAG_SLOP: f64 = 12.0;
struct RightFileDrag {
    index: usize,
    paths: Vec<PathBuf>,
    start: Point<Pixels>,
    active: bool,
}
impl Render for DragFiles {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The root paints a pointer-following operation label with the actual
        // destination. Keep GPUI's drag payload view transparent.
        div()
    }
}
enum Preview {
    Empty,
    Loading,
    Text(Arc<Vec<String>>, bool, &'static str),
    Image(PathBuf),
    Video(PathBuf),
    Pdf {
        path: PathBuf,
        page: u32,
        pages: u32,
    },
    Message(String),
}
enum Dialog {
    NewFolder,
    Shortcut,
    Rename(PathBuf),
    RenameVolume {
        path: PathBuf,
        initial: String,
    },
    /// Renaming "This PC": the inline editor sits on the navigation label.
    RenameComputer {
        initial: String,
    },
    Delete(Vec<PathBuf>),
    PermanentDelete(Vec<PathBuf>),
    Transfer {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        copy_only: bool,
        mode: shell::TransferMode,
    },
}
struct Marquee {
    origin: Point<Pixels>,
    current: Point<Pixels>,
    base: BTreeSet<usize>,
    indices: HashMap<PathBuf, usize>,
    toggle: bool,
    active: bool,
}
/// A retained row view. The pool is bounded by the viewport, never directory size.
/// Rebinding replaces every event handler and visual property via the normal renderer.
struct RecycledRow {
    owner: WeakEntity<Explorer>,
    index: usize,
}
impl Render for RecycledRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.owner
            .update(cx, |owner, cx| {
                owner.render_list_row(self.index, window, cx)
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

struct SystemOperation {
    id: u64,
    label: String,
    started: Instant,
    finished: Option<Instant>,
    status: String,
    control: shell::OperationControl,
}
struct Explorer {
    system_operations: Vec<SystemOperation>,
    next_operation: u64,
    marquee: Option<Marquee>,
    settings: Settings,
    options: Option<Settings>,
    options_page: usize,
    association_busy: bool,
    options_scroll: ScrollHandle,
    toolbar_overflow: Vec<&'static str>,
    nested_submenu: Option<(String, f32)>,
    popup: Option<(String, Point<Pixels>)>,
    submenu: Option<(String, f32)>,
    columns_resize: Option<(usize, f32, f32)>,
    volume_columns_resize: Option<(usize, f32, f32)>,
    sidebar_resize: Option<(f32, f32)>,
    text_menu_target: Option<Entity<TextInput>>,
    preview_resize: Option<(f32, f32)>,
    scroll_drag: Option<(bool, f32, f32, f32)>,
    aux_scroll_drag: Option<(ScrollHandle, f32, f32, f32)>,
    aux_horizontal_drag: Option<(ScrollHandle, f32, f32, f32)>,
    watch_pending: bool,
    content_scroll: ScrollHandle,
    home_scroll: ScrollHandle,
    high_performance_row: std::cell::Cell<f32>,
    high_performance_visible: std::cell::Cell<usize>,
    rows: Vec<DisplayRow>,
    content_image_present: bool,
    thumbnails: rust_explorer::thumbnails::Loader,
    thumbnail_images: Entity<visual::ThumbnailImages>,
    thumbnail_viewport: std::cell::Cell<Option<(u64, usize, usize, u32, u64)>>,
    thumbnail_sync_queued: std::cell::Cell<bool>,
    hidden_breadcrumbs: std::cell::RefCell<Vec<(String, PathBuf)>>,
    view: ViewMode,
    group: String,
    filters: rust_explorer::file_filter::Filters,
    filter_types: Vec<String>,
    filter_folder: PathBuf,
    group_members: std::collections::BTreeMap<String, Vec<usize>>,
    collapsed_groups: BTreeSet<String>,
    usage: HashMap<PathBuf, Usage>,
    usage_cancel: Arc<AtomicBool>,
    usage_rx: mpsc::Receiver<(u64, PathBuf, Usage)>,
    usage_tx: mpsc::Sender<(u64, PathBuf, Usage)>,
    usage_running: bool,
    usage_revision: u64,
    usage_enabled: bool,
    inline_rename: Option<PathBuf>,
    pending_select: Option<PathBuf>,
    external_reveal: Vec<PathBuf>,
    reveal_pending: bool,
    reveal_scroll: Option<PathBuf>,
    pending_rename: bool,
    /// Explicit rename target coming from a context-menu command.
    pending_rename_path: Option<PathBuf>,
    rename_token: u64,
    selected_at: Instant,
    templates: Vec<new_file::Template>,
    drag_rx: mpsc::Receiver<TabDrop>,
    drag_tx: mpsc::Sender<TabDrop>,
    own_window: Option<AnyWindowHandle>,
    tab_drag: Option<Arc<AtomicBool>>,
    tab_tip: Option<crate::tab_tip::TabTip>,
    /// Scale the tab card was built for (zoom × monitor DPI).
    tab_tip_scale: f32,
    volumes: Vec<Volume>,
    storage_volumes: Vec<Volume>,
    volume_busy: bool,
    volume_tick: Instant,
    volume_capacity_tick: Instant,
    system_quick_access: Vec<rust_explorer::quick_access::Item>,
    quick_access_tick: Instant,
    quick_access_busy: bool,
    computer_open: bool,
    computer_name: String,
    computer_name_tick: Instant,
    computer_name_busy: bool,
    tree_open: BTreeSet<PathBuf>,
    tree_children: HashMap<PathBuf, Result<Vec<(String, PathBuf)>, String>>,
    tree_loading: BTreeSet<PathBuf>,
    tree_hidden: BTreeSet<PathBuf>,
    nav_scroll: UniformListScrollHandle,
    tab_scroll: ScrollHandle,
    device_groups: BTreeSet<String>,
    context_target: Option<PathBuf>,
    theme: Theme,
    theme_tick: Instant,
    tabs: Vec<Tab>,
    active: usize,
    entries: Arc<Vec<Entry>>,
    selected: BTreeSet<usize>,
    keyboard_cursor: Option<PathBuf>,
    keyboard_focus_region: u8,
    view_wheel_remainder: f32,
    focus_item: Option<PathBuf>,
    pending_selection_click: Option<usize>,
    ctrl_open_selection: Option<(BTreeSet<usize>, Instant)>,
    /// View state to apply once the listing of a back/forward/up target loads.
    pending_restore: Option<rust_explorer::fs::HistoryState>,
    /// Scroll row to restore when nothing was selected in the target folder.
    pending_scroll_row: Option<f32>,
    file_drag: Option<FileDragState>,
    right_file_drag: Option<RightFileDrag>,
    pending_right_drop: Option<(Vec<PathBuf>, PathBuf)>,
    file_drop_bounds: std::rc::Rc<std::cell::RefCell<Vec<(PathBuf, Bounds<Pixels>, bool)>>>,
    anchor: usize,
    sort: Sort,
    descending: bool,
    hidden: bool,
    loading: bool,
    busy: bool,
    system_menu_active: bool,
    undo_stack: Vec<rust_explorer::file_history::Transaction>,
    redo_stack: Vec<rust_explorer::file_history::Transaction>,
    pending_history: Option<bool>,
    status: String,
    error: Option<String>,
    focus: FocusHandle,
    nav_focus: FocusHandle,
    nav_keyboard_index: usize,
    nav_expand_pending: Option<(PathBuf, usize)>,
    address: Entity<TextInput>,
    search: Entity<TextInput>,
    dialog_input: Entity<TextInput>,
    editing_address: bool,
    address_before_edit: String,
    address_candidates: Vec<rust_explorer::path_completion::Candidate>,
    address_choice: Option<usize>,
    address_generation: u64,
    address_scroll: ScrollHandle,
    address_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    recycled_rows: Vec<Entity<RecycledRow>>,
    shell_names: HashMap<PathBuf, String>,
    shell_chain: Vec<(String, PathBuf)>,
    shell_clipboard: Option<(Vec<PathBuf>, bool)>,
    shell_clipboard_seq: u32,
    list_scroll: ListState,
    // A refresh of the same tab/location keeps its viewport while the replacement
    // listing loads. Navigation and searches still start at the top.
    reload_scope: Option<(u64, PathBuf, String)>,
    zoom: f32,
    layout_width: f32,
    layout_mode: Option<(ViewMode, bool)>,
    window_width: f32,
    layout_height: f32,
    pdf_page: u32,
    preview_cancel: Arc<AtomicBool>,
    preview: Preview,
    preview_open: bool,
    preview_token: u64,
    dialog: Option<Dialog>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    watcher: Option<notify::RecommendedWatcher>,
    watch_rx: mpsc::Receiver<()>,
    watch_tx: mpsc::Sender<()>,
    places: Vec<(String, PathBuf)>,
    _subscriptions: Vec<Subscription>,
}
impl Drop for Explorer {
    fn drop(&mut self) {
        self.usage_cancel.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        self.preview_cancel.store(true, Ordering::Relaxed);
    }
}

/// How a folder should be listed: the view saved for it in the settings file,
/// otherwise the built-in default for that kind of folder. This is resolved every
/// time a folder is listed, so a window never depends on what another window used.
fn resolved_view(settings: &rust_explorer::settings::Settings, path: &std::path::Path) -> ViewMode {
    let saved = settings.folder_views.get(path).copied();
    let fallback = built_in_view(path);
    let resolved = if settings.remember_views {
        saved.unwrap_or(fallback)
    } else {
        fallback
    };
    view_trace(format!(
        "resolved folder={:?} remember={} saved={:?} default={:?} -> {:?}",
        path, settings.remember_views, saved, fallback, resolved
    ));
    resolved
}
fn built_in_view(path: &std::path::Path) -> ViewMode {
    if volumes::is_computer(path) {
        ViewMode::Tiles
    } else if path.to_string_lossy().starts_with("mtp-root:") {
        ViewMode::Tiles
    } else {
        // Explorer's own default for an ordinary folder.
        ViewMode::Details
    }
}

/// Diagnostic: how a folder's listing view was decided. Written to the menu trace
/// log when `RUST_EXPLORER_MENU_TRACE=1`.
fn view_trace(message: impl std::fmt::Display) {
    rust_explorer::shell::menu_trace(format!("view: {message}"));
}

impl Explorer {
    pub(super) fn begin_marquee(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.options.is_some() || self.dialog.is_some() || self.busy {
            return;
        }
        self.rename_token += 1;
        window.focus(&self.focus);
        let base = if event.modifiers.control || event.modifiers.shift {
            self.selected.clone()
        } else {
            BTreeSet::new()
        };
        let indices = self
            .visible_file_indices()
            .into_iter()
            .filter_map(|index| self.entries.get(index).map(|e| (e.path.clone(), index)))
            .collect();
        self.marquee = Some(Marquee {
            origin: event.position,
            current: event.position,
            base: base.clone(),
            indices,
            toggle: event.modifiers.control,
            active: false,
        });
        self.selected = base;
        self.clear_preview();
        cx.notify();
    }
    fn update_marquee(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let area = if self.is_home() {
            self.home_scroll.bounds()
        } else {
            self.content_scroll.bounds()
        };
        let Some(drag) = &mut self.marquee else {
            return;
        };
        drag.current = point(
            position.x.clamp(area.left(), area.right()),
            position.y.clamp(area.top(), area.bottom()),
        );
        if !drag.active && (drag.current - drag.origin).magnitude() < 3. {
            return;
        }
        drag.active = true;
        let left = drag.origin.x.min(drag.current.x);
        let right = drag.origin.x.max(drag.current.x);
        let top = drag.origin.y.min(drag.current.y);
        let bottom = drag.origin.y.max(drag.current.y);
        let hit: BTreeSet<_> = self
            .file_drop_bounds
            .borrow()
            .iter()
            .filter(|(_, bounds, _)| {
                bounds.right() > left
                    && bounds.left() < right
                    && bounds.bottom() > top
                    && bounds.top() < bottom
                    && bounds.intersects(&area)
            })
            .filter_map(|(path, _, _)| drag.indices.get(path).copied())
            .collect();
        let selected = if drag.toggle {
            drag.base.symmetric_difference(&hit).copied().collect()
        } else {
            drag.base.union(&hit).copied().collect()
        };
        if self.selected != selected {
            self.selected = selected;
            self.keyboard_cursor = None;
            cx.notify();
        }
        cx.notify();
    }
    fn new(
        window: &mut Window,
        launch: Option<rust_explorer::launch_request::LaunchRequest>,
        cx: &mut Context<Self>,
    ) -> Self {
        visual::initialize_image_cache(cx);
        let folder_handler = std::env::args_os()
            .nth(1)
            .is_some_and(|a| a == "--open-folder");
        let mut settings = Settings::load();
        let initial = launch
            .as_ref()
            .map(|r| r.folder.clone())
            .or_else(|| {
                std::env::args_os()
                    .nth(if folder_handler { 2 } else { 1 })
                    .map(PathBuf::from)
                    .map(|p| {
                        if folder_handler {
                            rust_explorer::default_browser::folder_argument(&p).unwrap_or(p)
                        } else {
                            p
                        }
                    })
                    .filter(|p| navigation::is_virtual(p) || p.is_dir())
            })
            .unwrap_or_else(|| match settings.startup_page.as_str() {
                "this-pc" => PathBuf::from("::ThisPC"),
                "quick-access" => PathBuf::from("::QuickAccess"),
                _ => shell::known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile)
                    .unwrap_or_else(|| PathBuf::from("C:\\")),
            });
        let address =
            cx.new(|cx| TextInput::new(initial.display().to_string(), "输入文件夹路径", cx));
        address.update(cx, |input, _| {
            input.single_line_scroll = true;
            input.click_to_end = false;
        });
        let search = cx.new(|cx| TextInput::new(String::new(), "搜索此文件夹", cx));
        let dialog_input = cx.new(|cx| TextInput::new(String::new(), "名称", cx));
        let mut subscriptions = vec![];
        subscriptions.push(
            cx.subscribe_in(
                &address,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::ContextMenu(position) => {
                        this.text_menu_target = Some(input.clone());
                        this.popup = Some(("text-menu".into(), *position));
                        cx.notify();
                    }
                    InputEvent::Submit => {
                        if let Some(choice) = this.address_choice {
                            if let Some(candidate) = this.address_candidates.get(choice).cloned() {
                                this.open_address_path(candidate.path, window, cx);
                                return;
                            }
                        }
                        let text = input.read(cx).value();
                        this.editing_address = false;
                        let typed = text.trim_matches('"').trim();
                        // Portable-device addresses are Shell namespace paths,
                        // not filesystem paths. Accept the displayed breadcrumb
                        // path and the explicit mtp: / mtp:// forms.
                        if let Some(hex) = typed.strip_prefix("mtp://") {
                            this.address_candidates.clear();
                            window.focus(&this.focus);
                            this.navigate(PathBuf::from(format!("mtp:{hex}")), cx);
                            return;
                        }
                        if typed.starts_with("mtp:") || typed.starts_with("mtp-root:") {
                            this.address_candidates.clear();
                            window.focus(&this.focus);
                            this.navigate(PathBuf::from(typed), cx);
                            return;
                        }
                        // Quick Access and the Recycle Bin have no filesystem
                        // path: the address bar shows their display name, so
                        // typing that name back has to return to the location
                        // instead of probing the disk. A real relative entry
                        // with the same name still wins.
                        if let Some(path) =
                            rust_explorer::shell_folder::resolve_named_location(typed)
                            && !rust_explorer::path_completion::resolve_input(&this.dir(), typed)
                                .is_some_and(|relative| relative.exists())
                        {
                            this.open_address_path(path, window, cx);
                            return;
                        }
                        let portable_display = typed.contains('\\')
                            && (typed.starts_with(&this.computer_name)
                                || typed.starts_with("此电脑")
                                // This PC's name is renamable, so a bin
                                // breadcrumb is recognized by its second part.
                                || rust_explorer::shell_folder::is_recycle_bin_address(typed)
                                || this.volumes.iter().any(|volume| {
                                    volume.kind == 7 && typed.starts_with(&volume.label)
                                }));
                        if portable_display {
                            this.address_candidates.clear();
                            window.focus(&this.focus);
                            let request = typed.to_string();
                            let task = cx.background_executor().spawn(async move {
                                let request = serde_json::to_string(&request).ok()?;
                                volumes::bounded::<PathBuf>(
                                    "--shell-resolve",
                                    &request,
                                    Duration::from_secs(5),
                                )
                                .ok()
                            });
                            cx.spawn(async move |view, cx| {
                                let path = task.await;
                                let _ = view.update(cx, |v, cx| match path {
                                    Some(path) => v.navigate(path, cx),
                                    None => {
                                        v.error = Some("找不到这个便携设备目录".into());
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                            return;
                        }
                        let path = match text.trim_matches('"') {
                            "此电脑" => volumes::computer(),
                            text if text == this.computer_name => volumes::computer(),
                            "网络" => PathBuf::from("::Network"),
                            "库" => PathBuf::from("::Libraries"),
                            text => {
                                rust_explorer::path_completion::resolve_input(&this.dir(), text)
                                    .unwrap_or_else(|| PathBuf::from(text))
                            }
                        };
                        this.open_address_path(path, window, cx);
                    }
                    InputEvent::Cancel => {
                        this.editing_address = false;
                        this.address_candidates.clear();
                        window.focus(&this.focus);
                        cx.notify();
                    }
                    InputEvent::Changed => {
                        this.update_address_candidates(input.read(cx).value(), cx)
                    }
                    InputEvent::Suggestion(delta) => {
                        let count = this.address_candidates.len();
                        if count > 0 {
                            let choice = match this.address_choice {
                                Some(index) => {
                                    (index as i32 + delta).rem_euclid(count as i32) as usize
                                }
                                None if *delta < 0 => count - 1,
                                None => 0,
                            };
                            this.address_choice = Some(choice);
                            this.address_scroll.scroll_to_item(choice);
                            cx.notify();
                        }
                    }
                },
            ),
        );
        subscriptions.push(
            cx.on_blur(&address.focus_handle(cx), window, |this, _, cx| {
                if this.address.read(cx).value().is_empty() {
                    let path = this.dir();
                    let value = this
                        .shell_names
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| path.display().to_string());
                    this.address
                        .update(cx, |input, cx| input.set_value(value, cx));
                }
                this.editing_address = false;
                cx.notify();
            }),
        );
        subscriptions.push(
            cx.subscribe_in(
                &search,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::ContextMenu(position) => {
                        this.text_menu_target = Some(input.clone());
                        this.popup = Some(("text-menu".into(), *position));
                        cx.notify();
                    }
                    InputEvent::Submit => {
                        this.tabs[this.active].query = input.read(cx).value();
                        window.focus(&this.focus);
                        this.reload(cx);
                    }
                    InputEvent::Changed => {
                        let query = input.read(cx).value();
                        this.tabs[this.active].query = query.clone();
                        this.cancel.store(true, Ordering::Relaxed);
                        this.generation += 1;
                        let generation = this.generation;
                        cx.spawn(async move |view, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(250))
                                .await;
                            view.update(cx, |this, cx| {
                                if this.generation == generation {
                                    this.reload(cx);
                                }
                            })
                            .ok();
                        })
                        .detach();
                    }
                    InputEvent::Cancel => {
                        this.tabs[this.active].query.clear();
                        this.search
                            .update(cx, |v, cx| v.set_value(String::new(), cx));
                        window.focus(&this.focus);
                        this.reload(cx);
                    }
                    InputEvent::Suggestion(_) => {}
                },
            ),
        );
        subscriptions.push(cx.subscribe_in(
            &dialog_input,
            window,
            |this, input, event, window, cx| match event {
                InputEvent::ContextMenu(position) => {
                    this.text_menu_target = Some(input.clone());
                    this.popup = Some(("text-menu".into(), *position));
                    cx.notify();
                }
                InputEvent::Submit => this.confirm_dialog(window, cx),
                InputEvent::Cancel => this.close_dialog(window, cx),
                InputEvent::Changed => {
                    if this.inline_rename.is_some() {
                        this.invalidate_list();
                        cx.notify();
                    }
                }
                InputEvent::Suggestion(_) => {}
            },
        ));
        let (watch_tx, watch_rx) = mpsc::channel();
        let (usage_tx, usage_rx) = mpsc::channel();
        let (drag_tx, drag_rx) = mpsc::channel();
        if !settings.quick_access_system_migrated {
            // Earlier releases seeded these six paths. System Quick Access now
            // supplies them; retain only folders the user added after that seed.
            let old_seed: Vec<_> = shell::places().into_iter().map(|(_, p)| p).collect();
            if settings.quick_access_initialized && settings.favorites.starts_with(&old_seed) {
                settings.favorites.drain(..old_seed.len());
            }
            settings.quick_access_initialized = true;
            settings.quick_access_system_migrated = true;
            settings.enqueue_save();
        }
        let mut view = resolved_view(&settings, &initial);
        if initial.to_string_lossy().starts_with("mtp-root:") {
            view = ViewMode::Tiles;
        }
        view_trace(format!(
            "new window initial={:?} remember={} -> view={:?}",
            initial, settings.remember_views, view
        ));
        let this = Self {
            marquee: None,
            view,
            options: None,
            options_page: 0,
            association_busy: false,
            options_scroll: ScrollHandle::new(),
            toolbar_overflow: vec![],
            nested_submenu: None,
            popup: None,
            submenu: None,
            columns_resize: None,
            volume_columns_resize: None,
            sidebar_resize: None,
            text_menu_target: None,
            preview_resize: None,
            scroll_drag: None,
            aux_scroll_drag: None,
            aux_horizontal_drag: None,
            watch_pending: false,
            content_scroll: ScrollHandle::new(),
            home_scroll: ScrollHandle::new(),
            high_performance_row: std::cell::Cell::new(0.),
            high_performance_visible: std::cell::Cell::new(0),
            thumbnails: rust_explorer::thumbnails::Loader::new(),
            thumbnail_images: visual::ThumbnailImages::new(cx),
            thumbnail_viewport: std::cell::Cell::new(None),
            thumbnail_sync_queued: std::cell::Cell::new(false),
            hidden_breadcrumbs: std::cell::RefCell::new(Vec::new()),
            rows: vec![],
            content_image_present: false,
            filters: Default::default(),
            filter_types: Vec::new(),
            filter_folder: PathBuf::new(),
            group_members: Default::default(),
            collapsed_groups: Default::default(),
            group: if volumes::is_computer(&initial) {
                "kind".into()
            } else {
                String::new()
            },
            usage: HashMap::new(),
            usage_cancel: Arc::new(AtomicBool::new(false)),
            usage_rx,
            usage_tx,
            usage_running: false,
            usage_revision: 0,
            usage_enabled: false,
            inline_rename: None,
            pending_select: None,
            reveal_pending: launch.as_ref().is_some_and(|r| !r.select.is_empty()),
            external_reveal: launch
                .as_ref()
                .map(|r| r.select.clone())
                .unwrap_or_default(),
            reveal_scroll: None,
            pending_rename: false,
            pending_rename_path: None,
            rename_token: 0,
            selected_at: Instant::now(),
            templates: vec![],
            drag_rx,
            drag_tx,
            own_window: Some(window.window_handle()),
            tab_drag: None,
            tab_tip: None,
            tab_tip_scale: 1.0,
            volumes: volumes::roots().iter().map(|p| volumes::basic(p)).collect(),
            storage_volumes: Vec::new(),
            volume_busy: false,
            volume_tick: Instant::now() - Duration::from_secs(10),
            volume_capacity_tick: Instant::now() - Duration::from_secs(60),
            system_quick_access: vec![],
            quick_access_tick: Instant::now() - Duration::from_secs(30),
            quick_access_busy: false,
            computer_open: true,
            computer_name: navigation::computer_name(),
            computer_name_tick: Instant::now() - Duration::from_secs(60),
            computer_name_busy: false,
            tree_open: Default::default(),
            tree_children: Default::default(),
            tree_loading: Default::default(),
            tree_hidden: Default::default(),
            nav_scroll: UniformListScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            device_groups: BTreeSet::new(),
            context_target: None,
            hidden: settings.hidden,
            preview_open: settings.preview,
            theme: Theme::preference(&settings.theme),
            theme_tick: Instant::now(),
            settings,
            tabs: vec![Tab {
                id: next_tab_id(),
                history: History::new(initial),
                query: String::new(),
                global: false,
                reveal_targets: launch.map(|r| r.select).unwrap_or_default(),
            }],
            active: 0,
            entries: Arc::new(vec![]),
            selected: BTreeSet::new(),
            keyboard_cursor: None,
            keyboard_focus_region: 0,
            view_wheel_remainder: 0.,
            focus_item: None,
            pending_selection_click: None,
            ctrl_open_selection: None,
            pending_restore: None,
            pending_scroll_row: None,
            file_drag: None,
            right_file_drag: None,
            pending_right_drop: None,
            file_drop_bounds: Default::default(),
            anchor: 0,
            sort: Sort::Name,
            descending: false,
            loading: false,
            busy: false,
            system_operations: Vec::new(),
            next_operation: 0,
            system_menu_active: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_history: None,
            status: String::new(),
            error: None,
            focus: cx.focus_handle(),
            nav_focus: cx.focus_handle(),
            nav_keyboard_index: usize::MAX,
            nav_expand_pending: None,
            address,
            search,
            dialog_input,
            editing_address: false,
            address_before_edit: String::new(),
            address_candidates: vec![],
            address_choice: None,
            address_generation: 0,
            address_scroll: ScrollHandle::new(),
            address_bounds: Default::default(),
            recycled_rows: vec![],
            shell_names: HashMap::new(),
            shell_chain: Vec::new(),
            shell_clipboard: None,
            shell_clipboard_seq: 0,
            list_scroll: ListState::new(0, ListAlignment::Top, px(160.)),
            reload_scope: None,
            zoom: 1.,
            layout_width: 0.,
            layout_mode: None,
            window_width: 1280.,
            layout_height: 0.,
            pdf_page: 0,
            preview_cancel: Arc::new(AtomicBool::new(false)),
            preview: Preview::Empty,
            preview_token: 0,
            dialog: None,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            watcher: None,
            watch_rx,
            watch_tx,
            places: shell::places(),
            _subscriptions: subscriptions,
        };
        let weak = cx.entity().downgrade();
        this.list_scroll.set_scroll_handler(move |_, _, cx| {
            let _ = weak.update(cx, |_, cx| cx.notify());
        });
        window.focus(&this.focus);
        let launch_hwnd = native_hwnd(window);
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(125))
                    .await;
                if view
                    .update(cx, |this, cx| {
                        if let Some(request) = launch_hwnd.and_then(rust_explorer::instance::take) {
                            this.accept_launch(request, cx);
                        }
                        // Windows performs live window moves on the UI thread.
                        // Pause background polls/icon notifications while the
                        // left button is held so they cannot force repaints
                        // during a drag.
                        if unsafe {
                            windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(1)
                        } < 0
                        {
                            return;
                        }
                        this.refresh_quick_access(cx);
                        this.refresh_computer_name(cx);
                        if this.volume_busy {
                            if let Some(snapshot) = volumes::cached_snapshot() {
                                if snapshot != this.volumes {
                                    this.volumes = snapshot;
                                    if this.is_home() {
                                        this.sync_computer_entries();
                                    }
                                    cx.notify();
                                }
                            }
                        }
                        if this
                            .popup
                            .as_ref()
                            .is_some_and(|(kind, _)| kind == "system-operations")
                            && this
                                .system_operations
                                .iter()
                                .any(|op| op.finished.is_none())
                        {
                            cx.notify();
                        }
                        if !this.volume_busy && this.volume_tick.elapsed() > Duration::from_secs(3)
                        {
                            this.refresh_volumes(cx);
                        }
                        let pending_tree: Vec<_> = this
                            .tree_open
                            .iter()
                            .filter(|p| {
                                !navigation::is_virtual(p)
                                    && !this.tree_children.contains_key(*p)
                                    && !this.tree_loading.contains(*p)
                            })
                            .take(4usize.saturating_sub(this.tree_loading.len()))
                            .cloned()
                            .collect();
                        for path in pending_tree {
                            this.load_tree(path, cx);
                        }
                        let mut usage_changed = false;
                        for (generation, path, usage) in this.usage_rx.try_iter() {
                            if generation == this.usage_revision {
                                this.usage.insert(path, usage);
                                usage_changed = true;
                            }
                        }
                        if usage_changed {
                            this.usage_running = this.usage_running
                                && this
                                    .entries
                                    .iter()
                                    .any(|e| e.directory && !this.usage.contains_key(&e.path));
                            this.rebuild_rows();
                            cx.notify();
                        }
                        let ended: Vec<_> = this.drag_rx.try_iter().collect();
                        for drop in ended {
                            this.tab_drag = None;
                            if !drop.accepted {
                                this.finish_tab_drag(drop, cx);
                            }
                        }
                        if visual::drain() | this.thumbnails.changed() {
                            cx.notify();
                        }
                        this.watch_pending |= this.watch_rx.try_iter().count() > 0;
                        if this.watch_pending
                            && this.dialog.is_none()
                            && this.options.is_none()
                            && !this.loading
                            && !this.busy
                            && this.tabs[this.active].query.is_empty()
                        {
                            this.watch_pending = false;
                            this.reload(cx);
                        }
                        // Registry reads are cheap individually but not at
                        // animation-frame frequency; system theme polling is
                        // low priority and can be throttled.
                        if this.settings.theme == "system"
                            && this.theme_tick.elapsed() >= Duration::from_secs(2)
                        {
                            this.theme_tick = Instant::now();
                            let theme = Theme::system();
                            if theme.dark != this.theme.dark {
                                this.theme = theme;
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let task = cx
            .background_executor()
            .spawn(async { new_file::templates() });
        cx.spawn(async move |view, cx| {
            let templates = task.await;
            let _ = view.update(cx, |v, cx| {
                v.templates = templates;
                cx.notify();
            });
        })
        .detach();
        this
    }
    fn dir(&self) -> PathBuf {
        self.tabs[self.active].history.current().to_path_buf()
    }
    fn is_home(&self) -> bool {
        navigation::is_app_virtual(&self.dir()) && self.tabs[self.active].query.trim().is_empty()
    }
    pub(crate) fn is_device_root(&self) -> bool {
        self.dir().to_string_lossy().starts_with("mtp-root:")
    }
    fn invalidate_list(&self) {
        let offset = self.list_scroll.logical_scroll_top();
        self.list_scroll.reset(self.rows.len());
        self.list_scroll.scroll_to(offset);
    }
    fn accept_launch(
        &mut self,
        request: rust_explorer::launch_request::LaunchRequest,
        cx: &mut Context<Self>,
    ) {
        // Preserve a rename or an open dialog instead of discarding user input.
        if self.inline_rename.is_some() || self.dialog.is_some() || self.options.is_some() {
            cx.defer(move |cx| {
                if let Err(error) = open_explorer_request(None, Some(request), cx) {
                    eprintln!("{error:#}");
                }
            });
            return;
        }
        self.popup = None;
        self.filters = Default::default();
        self.collapsed_groups.clear();
        if !self.loading && !self.selected.is_empty() {
            self.tabs[self.active].reveal_targets = self
                .selected
                .iter()
                .filter_map(|i| self.entries.get(*i).map(|e| e.path.clone()))
                .collect();
        }
        let index = rust_explorer::launch_request::find_reveal_tab(
            &request,
            self.tabs
                .iter()
                .enumerate()
                .map(|(i, t)| (i, t.history.current(), t.reveal_targets.as_slice())),
        );
        let index = index.unwrap_or_else(|| {
            self.tabs.push(Tab {
                id: next_tab_id(),
                history: History::new(request.folder),
                query: String::new(),
                global: false,
                reveal_targets: Vec::new(),
            });
            self.tabs.len() - 1
        });
        if !request.select.is_empty() {
            self.tabs[index].reveal_targets = request.select;
        }
        self.tabs[index].query.clear();
        self.switch_tab(index, cx);
        if let Some(handle) = self.own_window {
            let focus = self.focus.clone();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    window.activate_window();
                    window.focus(&focus);
                });
            });
        }
        cx.notify();
    }
    /// Resolve how the given folder should be listed and reset the grouping that
    /// belongs to that kind of folder. A window built for one folder and then
    /// pointed at another must not keep the first folder's presentation.
    pub(super) fn apply_listing_defaults(&mut self, path: &std::path::Path) {
        self.view = resolved_view(&self.settings, path);
        if rust_explorer::shell_folder::is_recycle_bin_path(path) {
            self.view = ViewMode::Details;
        }
        if path.to_string_lossy().starts_with("mtp-root:") {
            self.view = ViewMode::Tiles;
        }
        // Grouping is a property of the folder being listed, not of the window:
        // This PC groups by type, an ordinary folder starts ungrouped.
        self.group = if volumes::is_computer(path) {
            "kind".into()
        } else {
            String::new()
        };
        view_trace(format!(
            "listing defaults for {:?} -> view={:?} group={:?}",
            path, self.view, self.group
        ));
    }
    fn navigate(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.settings.remember_views {
            self.view = resolved_view(&self.settings, &path);
            view_trace(format!(
                "navigate {:?} -> view={:?} group={:?}",
                path, self.view, self.group
            ));
        }
        if rust_explorer::shell_folder::is_recycle_bin_path(&path) {
            self.view = ViewMode::Details;
            view_trace(format!("navigate {:?} is recycling bin -> Details", path));
        }
        if volumes::is_computer(&path) && !volumes::is_computer(&self.dir()) {
            self.group = "kind".into();
            if !self.settings.remember_views {
                self.view = ViewMode::Tiles;
            }
        } else if volumes::is_computer(&self.dir()) && !volumes::is_computer(&path) {
            self.group.clear();
        }
        if path.to_string_lossy().starts_with("mtp-root:") {
            // Portable-device roots show storage volumes like drive tiles.
            self.view = ViewMode::Tiles;
            self.group.clear();
        }
        // Remember where the folder we are leaving was scrolled to and what was
        // selected there; going back restores both, like Explorer.
        self.remember_history_state();
        self.pending_restore = None;
        self.pending_scroll_row = None;
        self.tabs[self.active].history.push(path);
        if self.settings.expand_current {
            self.expand_to_current(cx);
        }
        self.tabs[self.active].query.clear();
        self.search
            .update(cx, |v, cx| v.set_value(String::new(), cx));
        self.reload(cx);
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.dir();
        let scope = (
            self.tabs[self.active].id,
            path.clone(),
            self.tabs[self.active].query.clone(),
        );
        let preserve_viewport = self.reload_scope.as_ref() == Some(&scope);
        self.reload_scope = Some(scope);
        if self.tabs[self.active]
            .reveal_targets
            .iter()
            .any(|p| p.parent() != Some(path.as_path()))
        {
            self.tabs[self.active].reveal_targets.clear();
            self.external_reveal.clear();
            self.reveal_pending = false;
        }
        self.ctrl_open_selection = None;
        self.usage_cancel.store(true, Ordering::Relaxed);
        self.usage_cancel = Arc::new(AtomicBool::new(false));
        self.usage_revision = self.usage_revision.wrapping_add(1);
        self.usage.clear();
        self.usage_running = false;
        self.usage_enabled = false;
        self.inline_rename = None;
        if !preserve_viewport {
            self.content_scroll.set_offset(point(px(0.), px(0.)));
            self.high_performance_row.set(0.);
            self.home_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.dialog = None;
        self.popup = None;
        self.rename_token += 1;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let reveal = self.external_reveal.clone();
        self.generation += 1;
        let generation = self.generation;
        let path = self.dir();
        if !navigation::is_shell_folder(&path) {
            self.shell_chain.clear();
        }
        let address_value = if navigation::is_shell_folder(&path) {
            self.shell_names
                .get(&path)
                .cloned()
                .unwrap_or_else(|| path.display().to_string())
        } else if navigation::is_app_virtual(&path) {
            navigation::location_name(&path)
        } else {
            path.display().to_string()
        };
        self.address
            .update(cx, |v, cx| v.set_value(address_value, cx));
        self.sync_search_placeholder(cx);
        self.loading = true;
        self.error = None;
        self.selected.clear();
        self.focus_item = None;
        self.keyboard_cursor = None;
        if !preserve_viewport {
            self.entries = Arc::new(vec![]);
            self.list_scroll.reset(0);
        }
        self.clear_preview();
        let query = self.tabs[self.active].query.clone();
        if navigation::is_app_virtual(&path) && query.trim().is_empty() {
            // The virtual pages are built synchronously and have no listing to
            // restore into; drop any pending history state so a later refresh
            // cannot apply it to the wrong folder.
            self.pending_restore = None;
            self.pending_scroll_row = None;
            self.loading = false;
            self.watcher = None;
            self.sync_computer_entries();
            if path == PathBuf::from("::QuickAccess") {
                self.quick_access_tick = Instant::now() - Duration::from_secs(30);
                self.refresh_quick_access(cx);
                cx.notify();
                return;
            }
            self.volume_capacity_tick = Instant::now() - Duration::from_secs(60);
            self.refresh_volumes(cx);
            cx.notify();
            return;
        }
        let shell_folder = navigation::is_shell_folder(&path);
        let virtual_search = navigation::is_app_virtual(&path);
        let path = if virtual_search {
            PathBuf::from(format!(
                "{}\\",
                std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
            ))
        } else {
            path
        };
        let result_path = path.clone();
        let global = self.tabs[self.active].global || virtual_search;
        let hidden = self.hidden;
        let protected = self.settings.protected;
        let folders_first = self.settings.folders_first;
        let sort = self.sort;
        let descending = self.descending;
        let tx = self.watch_tx.clone();
        let task = cx.background_executor().spawn(async move {
            let result = (|| -> anyhow::Result<_> {
                if shell_folder {
                    let request = serde_json::to_string(&path)?;
                    let listing: rust_explorer::shell_folder::Listing =
                        volumes::bounded("--shell-list", &request, Duration::from_millis(8000))?;
                    let mut entries = listing.entries;
                    if !query.trim().is_empty() {
                        let needle = query.to_lowercase();
                        entries.retain(|entry| entry.name.to_lowercase().contains(&needle));
                    }
                    if !hidden {
                        entries.retain(|entry| !entry.hidden);
                    }
                    if !protected {
                        entries.retain(|entry| !entry.system);
                    }
                    fs::sort(&mut entries, sort, descending);
                    if !folders_first {
                        entries.sort_by(|a, b| compare_entries(a, b, sort, descending));
                    }
                    let status = format!("{} 个项目 · {}", entries.len(), listing.name);
                    return Ok((entries, status, None, Some((listing.name, listing.parents))));
                }
                let (mut entries, status) = if query.trim().is_empty() {
                    let (items, skipped) = fs::read_directory(&path, &cancel)?;
                    let n = items.len();
                    (
                        items,
                        format!(
                            "{n} 个项目{}",
                            if skipped > 0 {
                                format!(" · {skipped} 个项目无法读取")
                            } else {
                                String::new()
                            }
                        ),
                    )
                } else {
                    let result = search::query(&query, &path, global, &cancel)?;
                    let shown = result.entries.len();
                    let status = format!(
                        "{} · {} 个结果{}",
                        result.engine,
                        result.total,
                        if result.total as usize > shown {
                            format!(" · 显示前 {shown} 项，请缩小查询")
                        } else {
                            String::new()
                        }
                    );
                    (result.entries, status)
                };
                if !hidden {
                    entries.retain(|v| !v.hidden || reveal.contains(&v.path));
                }
                if !protected {
                    entries.retain(|e| !e.system || reveal.contains(&e.path));
                }
                fs::sort(&mut entries, sort, descending);
                if !folders_first {
                    entries.sort_by(|a, b| compare_entries(a, b, sort, descending));
                }
                let watcher = notify::recommended_watcher(
                    move |event: Result<notify::Event, notify::Error>| {
                        if let Ok(event) = event {
                            if !matches!(event.kind, notify::EventKind::Access(_)) {
                                let _ = tx.send(());
                            }
                        }
                    },
                )
                .ok()
                .and_then(|mut w| {
                    w.watch(&path, notify::RecursiveMode::NonRecursive)
                        .ok()
                        .map(|_| w)
                });
                Ok((entries, status, watcher, None))
            })();
            result
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            view.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((entries, status, watcher, shell_name)) => {
                        if let Some((name, parents)) = shell_name {
                            if !parents.is_empty() {
                                this.shell_chain = parents;
                            }
                            for (parent_name, parent_path) in &this.shell_chain {
                                this.shell_names
                                    .insert(parent_path.clone(), parent_name.clone());
                            }
                            this.shell_names.insert(result_path.clone(), name.clone());
                            let mut full_path = navigation::computer_name();
                            if this.shell_chain.is_empty() {
                                full_path.push('\\');
                                full_path.push_str(&name);
                            } else {
                                for (parent_name, _) in &this.shell_chain {
                                    full_path.push('\\');
                                    full_path.push_str(parent_name);
                                }
                            }
                            this.address
                                .update(cx, |input, cx| input.set_value(full_path, cx));
                            this.sync_search_placeholder(cx);
                        }
                        this.storage_volumes.clear();
                        if result_path.to_string_lossy().starts_with("mtp-root:") {
                            for entry in &entries {
                                if let (Some(total), Some(free)) = (entry.total, entry.free) {
                                    this.storage_volumes.push(Volume {
                                        path: entry.path.clone(),
                                        label: entry.name.clone(),
                                        kind: 8,
                                        total: Some(total),
                                        free: Some(free),
                                        filesystem: "MTP".into(),
                                        remote: None,
                                        read_only: false,
                                    });
                                }
                            }
                        }
                        this.entries = Arc::new(entries);
                        this.usage = disk_usage::cached_sizes(
                            this.entries
                                .iter()
                                .filter(|e| e.directory)
                                .map(|e| e.path.as_path()),
                        );
                        this.usage_enabled = !this.usage.is_empty();
                        this.rebuild_rows();
                        if this.reveal_pending {
                            for target in &this.external_reveal {
                                if let Some(index) =
                                    this.entries.iter().position(|e| &e.path == target)
                                {
                                    this.selected.insert(index);
                                    this.anchor = index;
                                    this.keyboard_cursor = Some(target.clone());
                                    this.focus_item = Some(target.clone());
                                    this.reveal_scroll = Some(target.clone());
                                }
                            }
                            this.reveal_pending = false;
                        }
                        if let Some(path) = this.pending_select.take() {
                            if let Some(index) = this
                                .entries
                                .iter()
                                .position(|e| e.path == path)
                                .filter(|index| this.visible_file_indices().contains(index))
                            {
                                this.selected.insert(index);
                                this.pending_rename = true;
                            }
                        }
                        // Back/forward/up: reselect what was selected here and
                        // scroll back to it (or to the remembered row).
                        if let Some(restore) = this.pending_restore.take() {
                            for path in &restore.selection {
                                if let Some(index) =
                                    this.entries.iter().position(|entry| &entry.path == path)
                                {
                                    this.selected.insert(index);
                                }
                            }
                            let focus = restore.focus.as_ref().and_then(|path| {
                                this.entries.iter().position(|entry| &entry.path == path)
                            });
                            if let Some(index) = focus {
                                this.selected.insert(index);
                                this.anchor = index;
                                this.focus_item = Some(this.entries[index].path.clone());
                                this.keyboard_cursor = this.focus_item.clone();
                                this.reveal_scroll = this.focus_item.clone();
                            } else if restore.row > 0. {
                                this.pending_scroll_row = Some(restore.row);
                            }
                        }
                        this.status = status;
                        this.watcher = watcher;
                    }
                    Err(e) => {
                        this.error = Some(format!("{e:#}"));
                        this.status = "加载失败 · F5 重试".into();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn clear_preview(&mut self) {
        self.preview_token += 1;
        self.preview = Preview::Empty;
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_cancel = Arc::new(AtomicBool::new(false));
    }
    fn paths(&self) -> Vec<PathBuf> {
        let visible = self.visible_file_indices();
        self.selected
            .iter()
            .filter(|index| visible.contains(index))
            .filter_map(|i| self.entries.get(*i).map(|v| v.path.clone()))
            .collect()
    }
    fn select(
        &mut self,
        index: usize,
        ctrl: bool,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = self.visible_file_order();
        if !order.contains(&index) {
            return;
        }
        if self.inline_rename.is_some() {
            self.confirm_dialog(window, cx);
            if self.inline_rename.is_some() {
                return;
            }
        }
        window.focus(&self.focus);
        self.rename_token += 1;
        self.selected_at = Instant::now();
        self.keyboard_cursor = None;
        if shift {
            self.selected = rust_explorer::file_filter::selection_range(&order, self.anchor, index)
                .into_iter()
                .collect();
        } else if ctrl {
            if !self.selected.remove(&index) {
                self.selected.insert(index);
            }
            self.anchor = index;
        } else {
            self.selected.clear();
            self.selected.insert(index);
            self.anchor = index;
        }
        self.pdf_page = 0;
        self.focus_item = self
            .entries
            .get(if self.selected.len() > 1 {
                *self.selected.first().unwrap()
            } else {
                index
            })
            .map(|e| e.path.clone());
        self.load_preview(cx);
        cx.notify();
    }
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        self.clear_preview();
        if !self.preview_open || self.selected.len() != 1 {
            return;
        }
        let Some(entry) = self
            .selected
            .first()
            .and_then(|i| self.entries.get(*i))
            .cloned()
        else {
            return;
        };
        if rust_explorer::shell_folder::is_recycle_bin_path(&entry.path) {
            self.preview = Preview::Message("回收站项目\n双击查看系统属性".into());
            return;
        }
        if entry.directory {
            self.preview = Preview::Message("文件夹\n双击打开".into());
            return;
        }
        let token = self.preview_token;
        let cancel = self.preview_cancel.clone();
        let page = self.pdf_page;
        self.preview = Preview::Loading;
        let task = cx.background_executor().spawn(async move {
            let value = (|| -> anyhow::Result<Preview> {
                let mut entry = entry;
                entry.path = preview::resolve_source(&entry.path, &cancel)?;
                let kind = preview::kind(&entry.path);
                Ok(match kind {
                    Kind::Text => {
                        let text = preview::text(&entry.path)?;
                        Preview::Text(Arc::new(text.lines), text.truncated, text.encoding)
                    }
                    Kind::Image => {
                        let size = std::fs::metadata(&entry.path)?.len();
                        if size > 64 * 1024 * 1024 {
                            Preview::Message("图片超过 64 MB，请使用默认应用打开".into())
                        } else {
                            Preview::Image(entry.path)
                        }
                    }
                    Kind::Video => {
                        match native_assets::preview(
                            &entry.path,
                            PreviewKind::Video,
                            0,
                            640,
                            &cancel,
                        ) {
                            Ok(asset) => Preview::Video(asset.path),
                            Err(_) => {
                                Preview::Message("此视频暂无可快速获取的系统单帧缩略图".into())
                            }
                        }
                    }
                    Kind::Pdf => {
                        let asset = native_assets::preview(
                            &entry.path,
                            PreviewKind::Pdf,
                            page,
                            1280,
                            &cancel,
                        )?;
                        Preview::Pdf {
                            path: asset.path,
                            page,
                            pages: asset.pages,
                        }
                    }
                    Kind::Font => {
                        let asset = native_assets::preview(
                            &entry.path,
                            PreviewKind::Font,
                            0,
                            960,
                            &cancel,
                        )?;
                        Preview::Image(asset.path)
                    }
                    Kind::Unsupported => {
                        Preview::Message("此文件类型不提供预览\n支持文本、照片、视频和 PDF".into())
                    }
                })
            })();
            value.unwrap_or_else(|e| Preview::Message(format!("无法预览\n{e:#}")))
        });
        cx.spawn(async move |view, cx| {
            let preview = task.await;
            view.update(cx, |this, cx| {
                if this.preview_token == token {
                    this.preview = preview;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        self.open_selected_with(Modifiers::default(), cx);
    }
    fn activate_file_item(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.click_count != 2 && !self.settings.single_click {
            if event.modifiers.control && self.selected.len() > 1 && self.selected.contains(&index)
            {
                self.ctrl_open_selection = Some((self.selected.clone(), Instant::now()));
            } else {
                self.ctrl_open_selection = None;
            }
            return false;
        }
        if event.modifiers.control {
            if let Some((selection, when)) = self.ctrl_open_selection.take() {
                if when.elapsed() < Duration::from_secs(1) && selection.contains(&index) {
                    self.selected = selection;
                }
            }
            self.selected.insert(index);
        } else {
            self.selected.clear();
            self.selected.insert(index);
        }
        self.pending_selection_click = None;
        self.rename_token += 1;
        self.open_selected_with(event.modifiers, cx);
        cx.notify();
        true
    }
    fn open_selected_with(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let disposition = rust_explorer::open_target::folder_disposition(
            modifiers.control,
            modifiers.shift,
            self.settings.shift_open_tab,
            self.settings.separate_process,
        );
        let entries: Vec<_> = self
            .selected
            .iter()
            .take(if modifiers.control { usize::MAX } else { 1 })
            .filter_map(|i| self.entries.get(*i))
            .cloned()
            .collect();
        for entry in entries {
            self.open_entry_with(entry, disposition, cx);
        }
    }
    /// Ctrl+double-click opens every selected file and opens every selected
    /// folder in its own window. Ctrl+single-click keeps toggling selection.
    fn open_tab_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.focus_existing_tab(&path, cx) {
            return;
        }
        self.tabs.push(Tab {
            id: next_tab_id(),
            history: History::new(path),
            query: String::new(),
            global: false,
            reveal_targets: Vec::new(),
        });
        self.switch_tab(self.tabs.len() - 1, cx);
    }
    /// When enabled, opening a folder in a new tab goes to a tab that already
    /// shows it instead of duplicating the folder.
    fn focus_existing_tab(&mut self, path: &std::path::Path, cx: &mut Context<Self>) -> bool {
        if !self.settings.focus_existing_tab {
            return false;
        }
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.history.current() == path)
        else {
            return false;
        };
        if index != self.active {
            self.switch_tab(index, cx);
        }
        true
    }
    fn open_entry(&mut self, entry: Entry, cx: &mut Context<Self>) {
        let disposition = rust_explorer::open_target::folder_disposition(
            false,
            false,
            false,
            self.settings.separate_process,
        );
        self.open_entry_with(entry, disposition, cx);
    }
    fn open_directory(
        &mut self,
        path: PathBuf,
        disposition: rust_explorer::open_target::OpenDisposition,
        cx: &mut Context<Self>,
    ) {
        use rust_explorer::open_target::OpenDisposition;
        let tab = Tab {
            id: next_tab_id(),
            history: History::new(path.clone()),
            query: String::new(),
            global: false,
            reveal_targets: Vec::new(),
        };
        match disposition {
            OpenDisposition::Current => self.navigate(path, cx),
            OpenDisposition::Tab => {
                if self.focus_existing_tab(&path, cx) {
                    return;
                }
                self.tabs.push(tab);
                self.switch_tab(self.tabs.len() - 1, cx);
            }
            OpenDisposition::Window => {
                if let Err(error) = open_explorer(Some(tab), cx) {
                    self.error = Some(error.to_string());
                    cx.notify();
                }
            }
        }
    }
    fn navigate_with_modifiers(
        &mut self,
        path: PathBuf,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let disposition = rust_explorer::open_target::folder_disposition(
            modifiers.control,
            modifiers.shift,
            self.settings.shift_open_tab,
            false,
        );
        let disposition = if self.dir() == PathBuf::from("::QuickAccess")
            && self.settings.quick_access_new_tab
            && disposition == rust_explorer::open_target::OpenDisposition::Current
        {
            rust_explorer::open_target::OpenDisposition::Tab
        } else {
            disposition
        };
        self.open_directory(path, disposition, cx);
    }
    fn open_entry_with(
        &mut self,
        entry: Entry,
        disposition: rust_explorer::open_target::OpenDisposition,
        cx: &mut Context<Self>,
    ) {
        if rust_explorer::shell_folder::is_recycle_bin_path(&entry.path) {
            self.run_shell(shell::Request::Properties { path: entry.path }, cx);
            return;
        }
        if entry.directory && rust_explorer::shell_folder::is_shell_path(&entry.path) {
            self.shell_names
                .insert(entry.path.clone(), entry.name.clone());
        }
        let disposition = if entry.directory
            && self.dir() == PathBuf::from("::QuickAccess")
            && self.settings.quick_access_new_tab
            && disposition == rust_explorer::open_target::OpenDisposition::Current
        {
            rust_explorer::open_target::OpenDisposition::Tab
        } else {
            disposition
        };
        if entry.directory && !entry.link {
            self.open_directory(entry.path, disposition, cx);
        } else if entry.link
            || preview::is_shortcut(&entry.path)
            || rust_explorer::open_target::is_font(&entry.path)
        {
            let source = entry.path;
            let generation = self.generation;
            let task = cx.background_executor().spawn(async move {
                let json = serde_json::to_string(&source)?;
                volumes::bounded::<rust_explorer::open_target::OpenTarget>(
                    "--open-target",
                    &json,
                    Duration::from_millis(1500),
                )
            });
            cx.spawn(async move |view, cx| {
                let result = task.await;
                let _ = view.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    match result {
                        Ok(rust_explorer::open_target::OpenTarget::Directory(path)) => {
                            v.open_directory(path, disposition, cx);
                        }
                        Ok(rust_explorer::open_target::OpenTarget::Font(path)) => {
                            v.run_shell(shell::Request::FontPreview { path }, cx)
                        }
                        Ok(rust_explorer::open_target::OpenTarget::File(path)) => {
                            v.run_shell(shell::Request::Open { path }, cx)
                        }
                        Err(error) => {
                            v.error = Some(format!("{error:#}"));
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        } else {
            self.run_shell(shell::Request::Open { path: entry.path }, cx);
        }
    }
    fn replay_history(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.busy || self.dialog.is_some() || self.options.is_some() {
            return;
        }
        let stack = if redo {
            &self.redo_stack
        } else {
            &self.undo_stack
        };
        if let Some(transaction) = stack.last().cloned() {
            self.pending_history = Some(redo);
            self.run_shell(shell::Request::History { transaction }, cx);
        }
    }
    fn limit_history(&mut self) -> bool {
        let mut released = false;
        while self
            .undo_stack
            .iter()
            .chain(self.redo_stack.iter())
            .map(rust_explorer::file_history::Transaction::approximate_bytes)
            .sum::<usize>()
            > 64 * 1024 * 1024
            || self.undo_stack.len() + self.redo_stack.len() > 32
        {
            if !self.undo_stack.is_empty() {
                self.undo_stack.remove(0);
            } else if !self.redo_stack.is_empty() {
                self.redo_stack.remove(0);
            } else {
                break;
            }
            released = true;
        }
        released
    }
    fn run_shell(&mut self, request: shell::Request, cx: &mut Context<Self>) {
        if self
            .system_operations
            .iter()
            .filter(|op| op.finished.is_none())
            .count()
            >= 8
        {
            self.error = Some("同时运行的系统操作过多，请先停止或关闭现有操作".into());
            cx.notify();
            return;
        }
        let invalidates_usage = matches!(
            &request,
            shell::Request::EmptyRecycleBin
                | shell::Request::History { .. }
                | shell::Request::RecordCreated { .. }
                | shell::Request::Copy { .. }
                | shell::Request::Transfer { .. }
                | shell::Request::Delete { .. }
                | shell::Request::PermanentDelete { .. }
                | shell::Request::Rename { .. }
                | shell::Request::Shortcut { .. }
                | shell::Request::Menu { .. }
                | shell::Request::ThemedMenu { .. }
        );
        let passive = matches!(
            &request,
            shell::Request::Open { .. }
                | shell::Request::FontPreview { .. }
                | shell::Request::SystemProperties
                | shell::Request::Share { .. }
                | shell::Request::Properties { .. }
                | shell::Request::PropertiesMany { .. }
                | shell::Request::SharingAccess { .. }
                | shell::Request::AdvancedShare { .. }
                | shell::Request::InvokeVerb { .. }
        );
        if self.busy && !passive {
            return;
        }
        let system_menu = matches!(
            &request,
            shell::Request::Menu { .. }
                | shell::Request::ThemedMenu { .. }
                | shell::Request::ComputerMenu { .. }
        );
        if system_menu {
            self.system_menu_active = true;
        }
        if !passive {
            self.busy = true;
        }
        self.next_operation += 1;
        let operation_id = self.next_operation;
        let control = shell::OperationControl::default();
        if self.system_operations.len() >= 32 {
            if let Some(index) = self
                .system_operations
                .iter()
                .position(|op| op.finished.is_some())
            {
                self.system_operations.remove(index);
            }
        }
        self.system_operations.push(SystemOperation {
            id: operation_id,
            label: shell::operation_label(&request),
            started: Instant::now(),
            finished: None,
            status: "正在进行".into(),
            control: control.clone(),
        });
        self.error = None;
        let recycle_menu = match &request {
            shell::Request::Menu { folder, .. } | shell::Request::ThemedMenu { folder, .. } => {
                rust_explorer::shell_folder::is_recycle_bin_path(folder)
            }
            _ => false,
        };
        let refresh = recycle_menu
            || !matches!(
                &request,
                shell::Request::Open { .. }
                    | shell::Request::FontPreview { .. }
                    | shell::Request::SystemProperties
                    | shell::Request::Share { .. }
                    | shell::Request::Properties { .. }
                    | shell::Request::PropertiesMany { .. }
                    | shell::Request::SharingAccess { .. }
                    | shell::Request::AdvancedShare { .. }
                    | shell::Request::InvokeVerb { .. }
                    | shell::Request::Menu { .. }
                    | shell::Request::ThemedMenu { .. }
            );
        let moving_paths = match &request {
            shell::Request::Copy {
                paths,
                moving: true,
                ..
            } => Some(paths.clone()),
            _ => None,
        };
        // A cancelled batch may already have deleted some items. Always list
        // again once the native operation finishes, including partial failures.
        let deletion = matches!(
            &request,
            shell::Request::Delete { .. } | shell::Request::PermanentDelete { .. }
        );
        let menu_paths = if let shell::Request::Menu { paths, folder, .. }
        | shell::Request::ThemedMenu { paths, folder, .. } = &request
        {
            if paths.is_empty() {
                vec![folder.clone()]
            } else {
                paths.clone()
            }
        } else {
            vec![]
        };
        let task = cx.background_executor().spawn(async move {
            let result = shell::execute_controlled(request, control);
            let mut history = result
                .as_ref()
                .ok()
                .and_then(|output| rust_explorer::file_history::decode(output));
            if let Some(response) = history.as_mut() {
                if !response.inverse.actions.is_empty()
                    && !response
                        .inverse
                        .request_bytes()
                        .is_ok_and(|bytes| bytes <= shell::MAX_REQUEST_BYTES)
                {
                    response.inverse.actions.clear();
                    response.warning =
                        Some("操作完成，但撤销记录超过传输上限，无法保留本次历史".into());
                }
            }
            (result, history)
        });
        cx.spawn(async move |view, cx| {
            let (result, history) = task.await;
            view.update(cx, |this, cx| {
                if let Some(op) = this
                    .system_operations
                    .iter_mut()
                    .find(|op| op.id == operation_id)
                {
                    op.finished = Some(Instant::now());
                    op.status = if op.control.stopping() {
                        "已停止（请确认部分操作结果）".into()
                    } else if let Err(error) = &result {
                        format!("失败：{error:#}")
                    } else if let Some(error) =
                        history.as_ref().and_then(|history| history.error.as_ref())
                    {
                        format!("失败：{error}")
                    } else {
                        "已完成".into()
                    };
                }
                if invalidates_usage {
                    disk_usage::invalidate_cache();
                }
                if system_menu {
                    this.system_menu_active = false;
                }
                if !passive {
                    this.busy = false;
                }
                if let Some(response) = history {
                    let direction = this.pending_history.take();
                    if response.warning.is_some() {
                        this.undo_stack.clear();
                        this.redo_stack.clear();
                    } else if let Some(redo) = direction {
                        if response.error.is_none() || !response.inverse.actions.is_empty() {
                            let source = if redo {
                                &mut this.redo_stack
                            } else {
                                &mut this.undo_stack
                            };
                            source.pop();
                            if let Some(remaining) = response.remaining {
                                if !remaining.actions.is_empty() {
                                    source.push(remaining);
                                }
                            }
                        }
                        if !response.inverse.actions.is_empty() {
                            let destination = if redo {
                                &mut this.undo_stack
                            } else {
                                &mut this.redo_stack
                            };
                            destination.push(response.inverse);
                        }
                    } else if !response.inverse.actions.is_empty() {
                        this.redo_stack.clear();
                        this.undo_stack.push(response.inverse);
                    }
                    let released_history = this.limit_history();
                    if response.error.is_none() {
                        if let Some(paths) = &moving_paths {
                            if shell::paste_from_clipboard()
                                .is_ok_and(|(current, cut)| cut && current == *paths)
                            {
                                shell::clear_clipboard();
                            }
                        }
                    }
                    this.reload(cx);
                    this.status = response.warning.unwrap_or_else(|| {
                        if direction == Some(true) {
                            "已重做"
                        } else if direction == Some(false) {
                            "已撤销"
                        } else {
                            "操作完成，可按 Ctrl+Z 撤销"
                        }
                        .into()
                    });
                    if released_history {
                        this.status.push_str("；部分撤销记录已达到内存上限并释放");
                    }
                    if let Some(error) = response.error {
                        this.error = Some(error);
                    }
                    cx.notify();
                    return;
                }
                if result.is_err() {
                    this.pending_history = None;
                }
                if let Some(name) = result
                    .as_ref()
                    .ok()
                    .and_then(|value| value.strip_prefix("computer-name:"))
                {
                    this.apply_computer_name(name.trim(), cx);
                    return;
                }
                if let Some(command) = result
                    .as_ref()
                    .ok()
                    .and_then(|s| s.strip_prefix("command:"))
                {
                    if command == "file-open" {
                        if let Some(path) = menu_paths.first().cloned() {
                            let task = cx
                                .background_executor()
                                .spawn(async move { Entry::from_path(path) });
                            cx.spawn(async move |view, cx| {
                                let result = task.await;
                                let _ = view.update(cx, |v, cx| match result {
                                    Ok(entry) => v.open_entry(entry, cx),
                                    Err(error) => {
                                        v.error = Some(error.to_string());
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                        }
                        return;
                    }
                    let command = command.to_owned();
                    let entity = cx.entity().downgrade();
                    let own = this.own_window;
                    cx.defer(move |cx| {
                        if let Some(own) = own {
                            let _ = own.update(cx, |_, w, cx| {
                                let _ = entity.update(cx, |v, cx| v.menu_command(&command, w, cx));
                            });
                        }
                    });
                    cx.notify();
                    return;
                }
                if result.as_ref().is_ok_and(|s| s == "rename") {
                    if let Some(path) = menu_paths.first() {
                        this.pending_rename_path = Some(path.clone());
                        if let Some(index) = this.entries.iter().position(|e| &e.path == path) {
                            this.selected.clear();
                            this.selected.insert(index);
                        } else if path.parent().is_some_and(|parent| parent != path) {
                            // Reveal the item where it lives, then rename it there.
                            this.pending_select = Some(path.clone());
                            this.navigate(path.parent().unwrap_or(path).to_path_buf(), cx);
                            return;
                        }
                    }
                    this.pending_rename = true;
                    cx.notify();
                    return;
                }
                if result.as_ref().is_ok_and(|s| s == "properties") {
                    let mut paths = menu_paths.clone();
                    if paths.is_empty() {
                        paths.push(this.dir());
                    }
                    this.run_shell(shell::Request::PropertiesMany { paths }, cx);
                    cx.notify();
                    return;
                }
                let native_refresh = result.as_ref().is_ok_and(|value| value == "refresh");
                if let Err(e) = result {
                    this.error = Some(e.to_string());
                    if deletion {
                        this.reload(cx);
                    }
                    cx.notify();
                } else {
                    if let Some(paths) = moving_paths {
                        if shell::paste_from_clipboard()
                            .is_ok_and(|(current, cut)| cut && current == paths)
                        {
                            shell::clear_clipboard();
                        }
                    }
                    if refresh || native_refresh {
                        this.reload(cx);
                    } else {
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn menu(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        rust_explorer::shell::menu_trace(format!(
            "app: menu() rb_down={}",
            (unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_RBUTTON.0 as i32,
                )
            } as u16
                & 0x8000)
                != 0
        ));
        // The hover card must never overlap the menu it was opened next to.
        rust_explorer::file_tip::hide_current();
        if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
            self.system_menu(cx);
            return;
        }
        // The native MTP namespace menu is incomplete and can misbehave. Use
        // the app-owned item menu for portable-device items even when the
        // "full system menu" option is enabled.
        if navigation::is_shell_folder(&self.dir())
            && !rust_explorer::shell_folder::is_recycle_bin_path(&self.dir())
            && !self.selected.is_empty()
        {
            self.popup = Some(("file-menu".into(), point(px(240.), px(200.))));
            self.submenu = None;
            cx.notify();
            return;
        }
        if self
            .paths()
            .iter()
            .any(|path| volume_label::is_drive_root(path))
        {
            self.system_menu(cx);
            return;
        }
        if self.settings.full_system_menu
            && !(navigation::is_virtual(&self.dir()) && self.selected.is_empty())
        {
            self.system_menu(cx);
            return;
        }
        self.popup = Some((
            if self.selected.is_empty() {
                if self.dir() == PathBuf::from("::QuickAccess") {
                    "quick-background-menu"
                } else if navigation::is_app_virtual(&self.dir()) {
                    "computer-menu"
                } else {
                    "background-menu"
                }
            } else {
                "file-menu"
            }
            .into(),
            point(px(240.), px(200.)),
        ));
        self.submenu = None;
        cx.notify();
    }
    fn system_menu(&mut self, cx: &mut Context<Self>) {
        rust_explorer::shell::menu_trace(format!(
            "app: system_menu() rb_down={}",
            (unsafe {
                windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_RBUTTON.0 as i32,
                )
            } as u16
                & 0x8000)
                != 0
        ));
        rust_explorer::file_tip::hide_current();
        if navigation::is_shell_folder(&self.dir())
            && !rust_explorer::shell_folder::is_recycle_bin_path(&self.dir())
            && !self.selected.is_empty()
        {
            self.popup = Some(("file-menu".into(), point(px(240.), px(200.))));
            self.submenu = None;
            cx.notify();
            return;
        }
        if volumes::is_computer(&self.dir()) && self.selected.is_empty() {
            let (x, y) = shell::cursor();
            self.context_target = Some(volumes::computer());
            self.run_shell(
                shell::Request::ComputerMenu {
                    x,
                    y,
                    dark: self.theme.dark,
                    expanded: self.computer_open,
                    owner: shell::active_window_owner(),
                },
                cx,
            );
            return;
        }
        if navigation::is_app_virtual(&self.dir()) && self.selected.is_empty() {
            self.popup = Some((
                if self.dir() == PathBuf::from("::QuickAccess") {
                    "quick-background-menu"
                } else {
                    "computer-menu"
                }
                .into(),
                point(px(250.), px(220.)),
            ));
            self.submenu = None;
            cx.notify();
            return;
        }
        let (x, y) = shell::cursor();
        let paths = self.paths();
        // The Shell's file-system menu always offers `pintohome`; tell the host when
        // that folder is already pinned so it can present the unpin action instead.
        let quick_access_pinned = paths.len() == 1
            && self
                .system_quick_access
                .iter()
                .any(|item| item.path == paths[0] && item.pinned == Some(true));
        self.run_shell(
            shell::Request::ThemedMenu {
                paths,
                folder: self.dir(),
                x,
                y,
                dark: self.theme.dark,
                view: ViewMode::ALL
                    .iter()
                    .position(|(view, _)| *view == self.view)
                    .unwrap_or(5) as u8,
                sort: match self.sort {
                    Sort::Name => "name",
                    Sort::Modified => "modified",
                    Sort::Kind => "kind",
                    Sort::Size => "size",
                    Sort::Created => "created",
                    Sort::Original => "original",
                    Sort::Deleted => "deleted",
                }
                .into(),
                descending: self.descending,
                group: self.group.clone(),
                undo: !self.undo_stack.is_empty(),
                redo: !self.redo_stack.is_empty(),
                can_paste: self.can_paste(),
                quick_access_pinned,
                owner: shell::active_window_owner(),
            },
            cx,
        );
    }
    fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
        let paths = self.paths();
        if paths
            .iter()
            .any(|path| rust_explorer::shell_folder::is_shell_path(path))
        {
            self.shell_clipboard = Some((paths, cut));
            self.shell_clipboard_seq = shell::clipboard_sequence();
            self.status = if cut {
                "已剪切便携设备项目 · Ctrl+V 移动到目标文件夹"
            } else {
                "已复制便携设备项目 · Ctrl+V 粘贴"
            }
            .into();
            cx.notify();
            return;
        }
        self.shell_clipboard = None;
        match shell::copy_to_clipboard(&paths, cut) {
            Ok(()) => {
                self.status = if cut {
                    "已剪切 · Ctrl+V 移动到目标文件夹"
                } else {
                    "已复制 · Ctrl+V 粘贴"
                }
                .into()
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }
    pub(crate) fn can_paste(&self) -> bool {
        // The device storage selector is not a writable folder. The storage
        // root itself is also intentionally not a paste target.
        if self.is_device_root()
            || (self.dir().to_string_lossy().starts_with("mtp:") && self.shell_chain.len() == 2)
        {
            return false;
        }
        self.shell_clipboard.is_some() || shell::has_file_clipboard()
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        // A native Shell Copy/Cut writes the system clipboard. If that happened
        // after an MTP copy, the internal PIDL clipboard is stale and must not
        // win over the files the user just copied from a traditional menu.
        if self.shell_clipboard.is_some() && shell::clipboard_sequence() != self.shell_clipboard_seq
        {
            self.shell_clipboard = None;
        }
        if navigation::is_app_virtual(&self.dir()) || !self.can_paste() {
            if self.is_device_root()
                || (self.dir().to_string_lossy().starts_with("mtp:") && self.shell_chain.len() == 2)
            {
                self.status = "便携设备存储根目录不允许粘贴".into();
            }
            cx.notify();
            return;
        }
        if let Some((paths, moving)) = self.shell_clipboard.clone() {
            if !paths.is_empty() {
                self.run_shell(
                    shell::Request::Copy {
                        paths,
                        destination: self.dir(),
                        moving,
                    },
                    cx,
                );
            }
            if moving {
                self.shell_clipboard = None;
            }
            return;
        }
        match shell::paste_from_clipboard() {
            Ok((paths, moving)) => self.run_shell(
                shell::Request::Copy {
                    paths,
                    destination: self.dir(),
                    moving,
                },
                cx,
            ),
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
            }
        }
    }
    fn show_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        if navigation::is_virtual(&self.dir())
            && (self.is_home() || matches!(dialog, Dialog::NewFolder | Dialog::Shortcut))
        {
            return;
        }
        if matches!(dialog, Dialog::NewFolder) {
            if self.busy {
                return;
            }
            self.busy = true;
            let parent = self.dir();
            let original_parent = parent.clone();
            let task = cx
                .background_executor()
                .spawn(async move { fs::create_numbered_folder(&parent) });
            cx.spawn(async move |view, cx| {
                let result = task.await;
                let _ = view.update(cx, |this, cx| {
                    this.busy = false;
                    match result {
                        Ok(path) => {
                            if this.dir() == original_parent {
                                this.pending_select = Some(path.clone());
                            }
                            this.run_shell(shell::Request::RecordCreated { path }, cx);
                        }
                        Err(error) => {
                            this.error = Some(error.to_string());
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
            cx.notify();
            return;
        }
        let name = match &dialog {
            Dialog::RenameVolume { initial, .. } => initial.clone(),
            Dialog::RenameComputer { initial } => initial.clone(),
            Dialog::Rename(path) => {
                if rust_explorer::shell_folder::is_shell_path(path) {
                    self.selected
                        .iter()
                        .filter_map(|index| self.entries.get(*index))
                        .find(|entry| &entry.path == path)
                        .map(|entry| entry.name.clone())
                        .unwrap_or_else(|| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                } else {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }
            }
            Dialog::NewFolder => "新建文件夹".into(),
            Dialog::Shortcut => String::new(),
            Dialog::Delete(_) | Dialog::PermanentDelete(_) | Dialog::Transfer { .. } => {
                String::new()
            }
        };
        match &dialog {
            Dialog::Rename(path) => self.inline_rename = Some(path.clone()),
            Dialog::RenameVolume { path, .. } => self.inline_rename = Some(path.clone()),
            Dialog::RenameComputer { .. } => {
                self.inline_rename = Some(rust_explorer::volumes::computer())
            }
            _ => {}
        }
        self.dialog = Some(dialog);
        let directory = if let Some(Dialog::Rename(path)) = &self.dialog {
            self.entries
                .iter()
                .find(|e| &e.path == path)
                .is_some_and(|e| e.directory)
        } else {
            false
        };
        self.dialog_input.update(cx, |v, cx| {
            v.compact = matches!(&self.dialog, Some(Dialog::Rename(_)));
            v.double_click_select_all = matches!(
                &self.dialog,
                Some(
                    Dialog::Rename(_) | Dialog::RenameVolume { .. } | Dialog::RenameComputer { .. }
                )
            );
            v.set_value(name, cx);
            if matches!(&self.dialog, Some(Dialog::Rename(_))) {
                v.focus_filename(window, cx, directory);
            } else {
                v.focus_select_all(window, cx);
            }
        });
        cx.notify();
    }
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.inline_rename = None;
        window.focus(&self.focus);
        cx.notify();
    }
    fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.dialog_input.read(cx).value();
        if let Some(Dialog::RenameVolume { path, .. }) = &self.dialog {
            let filesystem = if volume_label::is_network_drive(path) {
                "NETWORK"
            } else {
                ""
            };
            if let Err(error) = volume_label::validate(&name, filesystem) {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        }
        if !matches!(
            self.dialog,
            Some(
                Dialog::Delete(_)
                    | Dialog::PermanentDelete(_)
                    | Dialog::Shortcut
                    | Dialog::Transfer { .. }
                    | Dialog::RenameVolume { .. }
            )
        ) {
            if let Err(e) = fs::valid_name(&name) {
                self.error = Some(e.to_string());
                cx.notify();
                return;
            }
        }
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        self.close_dialog(window, cx);
        match dialog {
            Dialog::Shortcut => {
                let target = PathBuf::from(name.trim_matches('"'));
                let name = target.file_stem().unwrap_or_default().to_string_lossy();
                let destination = (0..10000)
                    .map(|i| {
                        self.dir().join(if i == 0 {
                            format!("{name} - 快捷方式.lnk")
                        } else {
                            format!("{name} - 快捷方式 ({i}).lnk")
                        })
                    })
                    .find(|p| !p.exists());
                if let Some(destination) = destination {
                    self.run_shell(
                        shell::Request::Shortcut {
                            target,
                            destination,
                        },
                        cx,
                    );
                }
            }
            Dialog::Delete(paths) => self.run_shell(shell::Request::Delete { paths }, cx),
            Dialog::PermanentDelete(paths) => {
                self.run_shell(shell::Request::PermanentDelete { paths }, cx)
            }
            Dialog::Transfer {
                paths,
                destination,
                copy_only,
                mode,
            } => self.run_shell(
                shell::Request::Transfer {
                    paths,
                    destination,
                    copy_only,
                    mode,
                },
                cx,
            ),
            Dialog::Rename(path) => {
                if path
                    .file_name()
                    .is_some_and(|original| original == std::ffi::OsStr::new(&name))
                {
                    return;
                }
                self.run_shell(shell::Request::Rename { path, name }, cx)
            }
            Dialog::RenameVolume { path, initial } => {
                if name != initial {
                    // The volume list refreshes every few seconds, so the new label
                    // appears once the elevated helper finishes.
                    self.run_shell(shell::Request::RenameVolume { path, label: name }, cx);
                }
            }
            Dialog::RenameComputer { initial } => {
                if name.trim() != initial {
                    self.run_shell(
                        shell::Request::RenameComputer {
                            dark: self.theme.dark,
                            name: Some(name),
                        },
                        cx,
                    );
                }
            }
            Dialog::NewFolder => {
                let path = self.dir().join(name);
                let created = path.clone();
                let task = cx
                    .background_executor()
                    .spawn(async move { std::fs::create_dir(path) });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    view.update(cx, |this, cx| match result {
                        Ok(()) => {
                            this.run_shell(shell::Request::RecordCreated { path: created }, cx)
                        }
                        Err(e) => {
                            this.error = Some(e.to_string());
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }
    /// The shared dialog input, rendered in place of an item's own label while an
    /// inline rename targets it.
    fn inline_editor(&self) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .h(ui(22.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.dialog_input.clone())
            .into_any_element()
    }
    fn rename_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A menu command names its target explicitly (the tree can rename a drive
        // that is not part of the current listing).
        if let Some(path) = self.pending_rename_path.take() {
            self.begin_inline_rename(path, window, cx);
            return;
        }
        // F2 in the navigation tree renames the focused drive or "This PC".
        if self.keyboard_focus_region == 3 {
            let target = self
                .nav_rows()
                .get(self.nav_keyboard_index)
                .map(|row| row.path.clone())
                .filter(|path| {
                    rust_explorer::volumes::is_computer(path) || volume_label::is_drive_root(path)
                });
            if let Some(path) = target {
                self.begin_inline_rename(path, window, cx);
                return;
            }
        }
        if self.selected.len() != 1 {
            return;
        }
        let Some(entry) = self
            .selected
            .first()
            .and_then(|index| self.entries.get(*index))
            .cloned()
        else {
            return;
        };
        self.begin_inline_rename(entry.path, window, cx);
    }
    /// Start an in-place rename: the editor appears at the item's own label (file
    /// rows, drive tiles and navigation rows all render `inline_rename`).
    fn begin_inline_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.options.is_some() || self.dialog.is_some() {
            return;
        }
        if rust_explorer::volumes::is_computer(&path) {
            let initial = self.computer_name.clone();
            self.show_dialog(Dialog::RenameComputer { initial }, window, cx);
        } else if volume_label::is_drive_root(&path) {
            let volume = self.volumes.iter().find(|volume| volume.path == path);
            if volume.is_some_and(|volume| volume.read_only) {
                self.error = Some("此驱动器是只读的".into());
                cx.notify();
                return;
            }
            let initial = volume
                .map(|volume| volume.label.clone())
                .unwrap_or_default();
            self.show_dialog(Dialog::RenameVolume { path, initial }, window, cx);
        } else {
            self.show_dialog(Dialog::Rename(path), window, cx);
        }
    }
    fn delete_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.paths();
        if !paths.is_empty() {
            // MTP has no recycle bin, and many removable drives do not either.
            // Confirm the irreversible path before the Shell operation starts.
            let permanent = paths
                .iter()
                .any(|path| rust_explorer::shell_folder::is_shell_path(path))
                || !shell::can_recycle(&paths);
            let dialog = if permanent {
                Dialog::PermanentDelete(paths)
            } else {
                Dialog::Delete(paths)
            };
            self.show_dialog(dialog, window, cx);
        }
    }
    fn new_tab(&mut self, cx: &mut Context<Self>) {
        self.tabs.push(Tab {
            id: next_tab_id(),
            history: History::new(if self.settings.new_tab_this_pc {
                PathBuf::from("::ThisPC")
            } else {
                self.dir()
            }),
            query: String::new(),
            global: false,
            reveal_targets: Vec::new(),
        });
        self.switch_tab(self.tabs.len() - 1, cx);
    }
    fn switch_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let was_computer = volumes::is_computer(&self.dir());
        self.active = index;
        self.external_reveal = self.tabs[index].reveal_targets.clone();
        self.reveal_pending = !self.external_reveal.is_empty();
        self.reveal_scroll = None;
        if self.settings.remember_views {
            self.view = resolved_view(&self.settings, &self.dir());
        }
        if !was_computer && volumes::is_computer(&self.dir()) {
            self.group = "kind".into();
        } else if was_computer && !volumes::is_computer(&self.dir()) {
            self.group.clear();
        }
        self.tab_scroll.scroll_to_item(index);
        let query = self.tabs[index].query.clone();
        self.search.update(cx, |v, cx| v.set_value(query, cx));
        self.reload(cx);
    }
    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs.len() == 1 {
            if let Some(handle) = self.own_window {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, w, _| w.remove_window());
                });
            }
            return;
        }
        self.tabs.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.switch_tab(self.active, cx);
    }
    fn key_move(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.key_navigate(NavigationStep::Vertical(delta), false, false, window, cx);
    }
    fn key_navigate(
        &mut self,
        step: NavigationStep,
        extend: bool,
        focus_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.entries.is_empty()
            || self.options.is_some()
            || self.dialog.is_some()
            || self.popup.is_some()
            || !self.focus.is_focused(window)
        {
            return;
        }
        let order = self.visible_file_order();
        let visible: std::collections::HashSet<_> = order.iter().copied().collect();
        if order.is_empty() {
            return;
        }
        let current = self
            .keyboard_cursor
            .as_ref()
            .or(self.focus_item.as_ref())
            .and_then(|path| self.entries.iter().position(|entry| &entry.path == path))
            .filter(|index| visible.contains(index))
            .or_else(|| {
                self.selected
                    .iter()
                    .rev()
                    .copied()
                    .find(|index| visible.contains(index))
            });
        let has_current = current.is_some();
        let current = current.unwrap_or(order[0]);
        let columns = match self.view {
            ViewMode::ExtraLarge => (self.layout_width / 328.).floor().max(1.) as usize,
            ViewMode::Large => (self.layout_width / 164.).floor().max(1.) as usize,
            ViewMode::Medium => (self.layout_width / 124.).floor().max(1.) as usize,
            ViewMode::Small => (self.layout_width / 210.).floor().max(1.) as usize,
            ViewMode::Tiles => (self.layout_width / 290.).floor().max(1.) as usize,
            _ => 1,
        };
        let delta = match step {
            NavigationStep::Horizontal(delta) => {
                if self.view == ViewMode::List {
                    delta * self.rows.len().max(1) as isize
                } else if matches!(self.view, ViewMode::Details | ViewMode::Content) {
                    0
                } else {
                    delta
                }
            }
            NavigationStep::Vertical(delta) => {
                if self.view == ViewMode::List {
                    delta
                } else {
                    delta * columns as isize
                }
            }
            NavigationStep::Page(delta) => {
                delta
                    * ((self.layout_height / self.estimated_row_height())
                        .floor()
                        .max(1.) as isize)
                    * if self.view == ViewMode::List {
                        1
                    } else {
                        columns as isize
                    }
            }
            _ => 0,
        };
        let mut index = match step {
            NavigationStep::First => order[0],
            NavigationStep::Last => *order.last().unwrap(),
            _ => rust_explorer::file_filter::step_in_order(
                &order,
                has_current.then_some(current),
                delta,
            )
            .unwrap_or(order[0]),
        };
        // Follow the rendered grid, including grouped List columns whose
        // heading rows make arithmetic on raw entry indices inaccurate.
        let location = self
            .rows
            .iter()
            .enumerate()
            .find_map(|(row, items)| match items {
                DisplayRow::Items(items) => items
                    .iter()
                    .position(|item| *item == current)
                    .map(|column| (row, column)),
                DisplayRow::List(items) => items
                    .iter()
                    .position(|item| matches!(item, ListCell::File(value) if *value == current))
                    .map(|column| (row, column)),
                _ => None,
            });
        let cell = |row: usize, column: usize| -> Option<usize> {
            match self.rows.get(row)? {
                DisplayRow::Items(items) => items
                    .get(column.min(items.len().saturating_sub(1)))
                    .copied(),
                DisplayRow::List(items) => match items.get(column)? {
                    ListCell::File(index) => Some(*index),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some((row, column)) = location.filter(|_| has_current) {
            let geometry = match step {
                NavigationStep::Vertical(direction) | NavigationStep::Page(direction) => {
                    let distance = if matches!(step, NavigationStep::Page(_)) {
                        (self.layout_height / self.estimated_row_height())
                            .floor()
                            .max(1.) as isize
                    } else {
                        1
                    };
                    let start = (row as isize + distance * direction)
                        .clamp(0, self.rows.len().saturating_sub(1) as isize)
                        as usize;
                    if direction > 0 {
                        (start..self.rows.len()).find_map(|row| cell(row, column))
                    } else {
                        (0..=start).rev().find_map(|row| cell(row, column))
                    }
                }
                NavigationStep::Horizontal(direction) if self.view == ViewMode::List => {
                    let target = column as isize + direction;
                    if target < 0 {
                        Some(current)
                    } else {
                        cell(row, target as usize)
                            .or_else(|| {
                                (0..self.rows.len())
                                    .min_by_key(|candidate| {
                                        if cell(*candidate, target as usize).is_some() {
                                            candidate.abs_diff(row)
                                        } else {
                                            usize::MAX
                                        }
                                    })
                                    .and_then(|row| cell(row, target as usize))
                            })
                            .or(Some(current))
                    }
                }
                _ => None,
            };
            if let Some(target) = geometry {
                index = target;
            }
        }
        if !visible.contains(&index) {
            index = current;
        }
        if focus_only {
            self.focus_item = Some(self.entries[index].path.clone());
            window.focus(&self.focus);
        } else {
            self.select(index, false, extend, window, cx);
        }
        self.keyboard_cursor = Some(self.entries[index].path.clone());
        if let Some(row) = self
            .rows
            .iter()
            .position(|r| matches!(r,DisplayRow::Items(items) if items.contains(&index))
                || matches!(r, DisplayRow::List(cells) if cells.iter().any(|c| matches!(c,ListCell::File(i) if *i == index))))
        {
            if self.high_performance_active() {
                let top = self.high_performance_row.get().floor() as usize;
                let visible_rows = self.high_performance_visible.get().max(1);
                if row < top || row >= top.saturating_add(visible_rows) {
                    self.set_high_performance_row(row as f32);
                }
            } else {
                let top = self.list_scroll.logical_scroll_top().item_ix;
                let visible_rows =
                    (self.layout_height / self.estimated_row_height()).ceil() as usize + 1;
                // GPUI's reveal operation uses measured heights. Rows outside
                // the viewport have no height yet, so anchor distant jumps by
                // index.
                if row < top || row >= top.saturating_add(visible_rows) {
                    self.list_scroll
                        .scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
                } else {
                    self.list_scroll.scroll_to_reveal_item(row);
                }
            }
            if let DisplayRow::List(cells) = &self.rows[row] {
                if let Some(column) = cells.iter().position(|c| matches!(c,ListCell::File(i) if *i == index)) {
                    let left = column as f32 * 230. * self.zoom;
                    let viewport = self.layout_width * self.zoom;
                    let mut p = self.content_scroll.offset();
                    if left < -f32::from(p.x) {p.x = px(-left);}
                    else if left + 230. * self.zoom > -f32::from(p.x) + viewport { p.x = px(viewport - left - 230. * self.zoom); }
                    self.content_scroll.set_offset(p);
                }
            }
        }
        cx.notify();
    }
    /// First visible row, in row units, for the current view.
    fn scroll_row(&self) -> f32 {
        if self.high_performance_active() {
            self.high_performance_row.get()
        } else {
            let offset = self.list_scroll.logical_scroll_top();
            let height = (self.estimated_row_height() * self.zoom).max(1.);
            offset.item_ix as f32 + f32::from(offset.offset_in_item) / height
        }
    }
    fn history_state(&self) -> rust_explorer::fs::HistoryState {
        let focus = self.focus_item.clone().or_else(|| {
            self.selected
                .iter()
                .next()
                .and_then(|index| self.entries.get(*index))
                .map(|entry| entry.path.clone())
        });
        rust_explorer::fs::HistoryState {
            row: self.scroll_row(),
            focus,
            selection: self.paths(),
        }
    }
    /// Store the current scroll position and selection on the entry being left.
    fn remember_history_state(&mut self) {
        let state = self.history_state();
        self.tabs[self.active].history.remember(state);
    }
    fn apply_remembered_state(&mut self) {
        self.pending_restore = self.tabs[self.active].history.state();
        self.pending_scroll_row = None;
    }
    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.remember_history_state();
        if self.tabs[self.active].history.back() {
            self.apply_remembered_state();
            self.reload(cx);
        }
    }
    fn go_forward(&mut self, cx: &mut Context<Self>) {
        self.remember_history_state();
        if self.tabs[self.active].history.forward() {
            self.apply_remembered_state();
            self.reload(cx);
        }
    }
    fn can_go_up(&self) -> bool {
        !navigation::is_app_virtual(&self.dir())
    }
    /// Selecting the folder that was just left, the way Explorer does after `↑`.
    fn focus_after_load(&mut self, path: PathBuf) {
        if navigation::is_app_virtual(&self.dir()) {
            return;
        }
        self.pending_restore = Some(rust_explorer::fs::HistoryState {
            row: 0.,
            focus: Some(path.clone()),
            selection: vec![path],
        });
        self.pending_scroll_row = None;
    }
    /// The folder on the way to the current directory that sits directly under
    /// `target`. Navigating to an ancestor (breadcrumb click, `↑`) selects it,
    /// matching Explorer.
    fn child_towards_current(&self, target: &std::path::Path) -> Option<PathBuf> {
        let dir = self.dir();
        if navigation::is_shell_folder(&dir) {
            let index = self
                .shell_chain
                .iter()
                .position(|(_, path)| path == target)?;
            return self
                .shell_chain
                .get(index + 1)
                .map(|(_, path)| path.clone());
        }
        if volumes::is_computer(target) {
            let root = dir.ancestors().last().map(PathBuf::from)?;
            return (root != dir).then_some(root);
        }
        fs::child_towards(&dir, target)
    }
    fn go_up(&mut self, cx: &mut Context<Self>) {
        let path = self.dir();
        if navigation::is_app_virtual(&path) {
            return;
        }
        if navigation::is_shell_folder(&path) {
            if let Some(parent) =
                rust_explorer::shell_folder::parent_from_chain(&path, &self.shell_chain)
            {
                self.navigate(parent, cx);
                self.focus_after_load(path);
                return;
            }
            let generation = self.generation;
            let child = path.clone();
            let task = cx.background_executor().spawn(async move {
                let request = serde_json::to_string(&path)?;
                volumes::bounded::<PathBuf>("--shell-parent", &request, Duration::from_secs(5))
            });
            cx.spawn(async move |view, cx| {
                let result = task.await;
                let _ = view.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    match result {
                        Ok(parent) => {
                            v.navigate(parent, cx);
                            v.focus_after_load(child);
                        }
                        Err(error) => {
                            v.error = Some(error.to_string());
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        } else {
            self.navigate(
                path.parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(volumes::computer),
                cx,
            );
            self.focus_after_load(path);
        }
    }

    fn update_address_candidates(&mut self, text: String, cx: &mut Context<Self>) {
        let text = rust_explorer::path_completion::resolve_input(&self.dir(), &text)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.address_generation = self.address_generation.wrapping_add(1);
        let generation = self.address_generation;
        self.address_candidates.clear();
        self.address_choice = None;
        self.address_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
        cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            if !view
                .update(cx, |v, _| {
                    v.editing_address && v.address_generation == generation
                })
                .unwrap_or(false)
            {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let query = serde_json::to_string(&text).unwrap_or_default();
                    volumes::bounded::<Vec<rust_explorer::path_completion::Candidate>>(
                        "--path-completions",
                        &query,
                        Duration::from_millis(1000),
                    )
                })
                .await;
            let _ = view.update(cx, |v, cx| {
                if v.editing_address && v.address_generation == generation {
                    v.address_candidates = result.unwrap_or_default();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_address_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_address = false;
        self.address_candidates.clear();
        self.address_choice = None;
        self.address_generation = self.address_generation.wrapping_add(1);
        window.focus(&self.focus);
        if navigation::is_virtual(&path) {
            self.navigate(path, cx);
        } else {
            let task = cx
                .background_executor()
                .spawn(async move { Entry::from_path(path) });
            cx.spawn(async move |view, cx| {
                let result = task.await;
                let _ = view.update(cx, |v, cx| {
                    match result {
                        Ok(entry) => v.open_entry(entry, cx),
                        Err(error) => v.error = Some(error.to_string()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing_address {
            self.address_before_edit = self.address.read(cx).value();
        }
        self.editing_address = true;
        self.address_candidates.clear();
        self.address_choice = None;
        self.address_generation = self.address_generation.wrapping_add(1);
        self.address
            .update(cx, |v, cx| v.focus_select_all(window, cx));
        self.address_candidates.clear();
        cx.notify();
    }
    fn sync_search_placeholder(&self, cx: &mut Context<Self>) {
        let dir = self.dir();
        let scope = if navigation::is_shell_folder(&dir) {
            self.shell_names
                .get(&dir)
                .cloned()
                .unwrap_or_else(|| navigation::location_name(&dir))
        } else if self.tabs[self.active].global {
            if navigation::is_app_virtual(&dir) {
                navigation::location_name(&dir)
            } else {
                let root = dir.ancestors().last().unwrap_or(&dir).to_path_buf();
                self.volumes
                    .iter()
                    .find(|volume| volume.path == root)
                    .map(|volume| volume.label.clone())
                    .unwrap_or_else(|| navigation::location_name(&root))
            }
        } else {
            navigation::location_name(&dir)
        };
        self.search.update(cx, |input, cx| {
            input.set_placeholder(format!("在{scope}中搜索"), cx)
        });
    }
    fn change_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let offset = self.list_scroll.logical_scroll_top();
        self.zoom = if delta == 0. {
            1.
        } else {
            (self.zoom + delta).clamp(0.75, 2.)
        };
        window.set_rem_size(px(16. * self.zoom));
        self.invalidate_list();
        self.list_scroll.scroll_to(offset);
        cx.notify();
    }
    fn pdf_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Preview::Pdf { page, pages, .. } = &self.preview {
            let page = (*page as i32 + delta).clamp(0, *pages as i32 - 1) as u32;
            self.pdf_page = page;
            self.load_preview(cx);
            cx.notify();
        }
    }
}

/// Display name of a path, without the directory: dialogs mention the item only.
fn item_name(path: &std::path::Path, entries: &[Entry]) -> String {
    fs::dialog_item_name(path, entries)
}

/// First `limit` item names, one per line, with an ellipsis when there are more.
fn item_names(paths: &[std::path::PathBuf], limit: usize, entries: &[Entry]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .take(limit)
        .map(|path| item_name(path, entries))
        .collect();
    if paths.len() > limit {
        names.push("……".into());
    }
    names.join("\n")
}

fn icon(glyph: &str, color: u32) -> AnyElement {
    visual::icon(
        match glyph {
            "\u{e8b7}" => "folder",
            "\u{e721}" => "search",
            _ => "file",
        },
        color,
    )
}

fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, t: Theme) -> Stateful<Div> {
    let label: SharedString = label.into();
    let native = visual::button_name(label.as_ref());
    let symbolic = matches!(
        label.as_ref(),
        "←" | "→" | "↑" | "⟳" | "＋" | "×" | "‹" | "›"
    );
    let label: SharedString = match label.as_ref() {
        "⊕ 新建" => "新建".into(),
        "⌂ 主文件夹" => "主文件夹".into(),
        "▣ 预览" | "□ 预览" => "预览".into(),
        _ => label,
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .px_3()
        .h(ui(32.))
        .rounded(ui(4.))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(t.hover)))
        .when_some(native, |d, name| {
            d.child(if name == "refresh" {
                div()
                    .size(ui(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(ui(12.8))
                            .overflow_hidden()
                            .child(visual::icon_at(name, t.text, 12.8)),
                    )
                    .into_any_element()
            } else {
                visual::icon(name, t.text)
            })
        })
        .when(!symbolic, |d| d.child(label))
}

impl Render for Explorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.file_drop_bounds.borrow_mut().clear();
        window.set_rem_size(px(16. * self.zoom));
        visual::set_pixels((16. * self.zoom * window.scale_factor()).ceil() as u32);
        visual::set_theme(self.theme.dark);
        let width: f32 = window.viewport_size().width.into();
        let logical_width = width / self.zoom;
        self.window_width = logical_width;
        let sidebar_width = self
            .settings
            .sidebar_width
            .clamp(140., (logical_width * 0.48).min(600.).max(140.));
        let preview_width = self
            .settings
            .preview_width
            .clamp(180., (logical_width * 0.55).max(180.));
        let available = logical_width
            - sidebar_width
            - 5.
            - if self.preview_open || self.settings.details_pane {
                preview_width + 5.
            } else {
                0.
            };
        if self.pending_rename {
            self.pending_rename = false;
            self.rename_dialog(window, cx);
        }
        if self.settings.full_path {
            let title = if navigation::is_shell_folder(&self.dir()) {
                self.shell_names
                    .get(&self.dir())
                    .cloned()
                    .unwrap_or_else(|| navigation::location_name(&self.dir()))
            } else if navigation::is_app_virtual(&self.dir()) {
                navigation::location_name(&self.dir())
            } else {
                self.dir().display().to_string()
            };
            window.set_window_title(&title);
        } else {
            let path = self.dir();
            let title = self
                .shell_names
                .get(&path)
                .cloned()
                .unwrap_or_else(|| navigation::location_name(&path));
            window.set_window_title(&title);
        }
        let list_height = (f32::from(window.viewport_size().height) / self.zoom - 177.).max(100.);
        if (available - self.layout_width).abs() > 0.5
            || (list_height - self.layout_height).abs() > 0.5
            || self.layout_mode != Some((self.view, self.settings.compact))
        {
            let offset = self.list_scroll.logical_scroll_top();
            self.layout_width = available;
            self.layout_height = list_height;
            self.layout_mode = Some((self.view, self.settings.compact));
            self.rebuild_rows();
            self.list_scroll.scroll_to(offset);
        }
        if let Some(path) = self.reveal_scroll.take() {
            if let Some(index) = self.entries.iter().position(|e| e.path == path) {
                if let Some(row) = self.rows.iter().position(|r| matches!(r,DisplayRow::Items(items) if items.contains(&index)) || matches!(r,DisplayRow::List(cells) if cells.iter().any(|c| matches!(c,ListCell::File(i) if *i==index)))) {
                    self.set_high_performance_row(row as f32);
                    self.list_scroll.scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
                }
            }
        }
        if let Some(row) = self.pending_scroll_row.take() {
            // Nothing was selected in the folder we came back to: restore only the
            // scroll position.
            let max = self.rows.len().saturating_sub(1) as f32;
            let row = row.clamp(0., max);
            self.high_performance_row.set(row);
            self.set_list_scroll_row(row);
        }
        self.sync_thumbnails();
        let recycle = self.settings.high_performance
            && !matches!(
                self.view,
                ViewMode::Medium | ViewMode::Large | ViewMode::ExtraLarge
            )
            && !self.is_home();
        let pool_size = if recycle {
            // ListState measures up to 160 px on either side. 26 is the minimum
            // supported row height; two extra slots cover partial boundary rows.
            rust_explorer::layout_metrics::recycled_row_count(
                self.layout_height,
                self.zoom,
                self.rows.len(),
            )
        } else {
            0
        };
        if self.recycled_rows.len() != pool_size {
            self.recycled_rows = (0..pool_size)
                .map(|_| {
                    let owner = cx.entity().downgrade();
                    cx.new(|_| RecycledRow { owner, index: 0 })
                })
                .collect();
            self.invalidate_list();
        }
        self.toolbar_overflow = rust_explorer::settings::toolbar_overflow(
            logical_width
                - if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
                    128.
                } else {
                    0.
                },
            self.view == ViewMode::Details && !navigation::is_virtual(&self.dir()),
        );
        let t = self.theme;
        let mut root = div()
            .id("explorer")
            .capture_any_mouse_down(cx.listener(|v, e: &MouseDownEvent, w, _cx| {
                if v.popup
                    .as_ref()
                    .is_some_and(|(kind, _)| kind == "text-menu")
                {
                    return;
                }
                if v.editing_address {
                    let address = v.address_bounds.get();
                    let candidates = Bounds::new(
                        point(address.left(), address.bottom()),
                        size(
                            address.size.width,
                            ui(v.address_candidates.len().min(9) as f32 * 26.)
                                .to_pixels(w.rem_size()),
                        ),
                    );
                    if !address.contains(&e.position)
                        && (v.address_candidates.is_empty() || !candidates.contains(&e.position))
                    {
                        // Blank content/tool areas do not always claim focus themselves.
                        // Explicitly blur the editor so clearing without typing restores now.
                        w.focus(&v.focus);
                    }
                }
            }))
            .key_context("Explorer")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .font_family("Microsoft YaHei UI")
            .text_size(ui(13.))
            .line_height(ui(20.))
            .text_color(rgb(t.text))
            .bg(rgb(t.background))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, _, w, cx| {
                    if v.inline_rename.is_some() {
                        v.confirm_dialog(w, cx);
                    }
                }),
            )
            .on_action(cx.listener(|v, action: &NavigateFiles, w, cx| {
                v.key_navigate(action.step.clone(), action.extend, action.focus_only, w, cx);
            }))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|v, _, _, cx| {
                    if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                        return;
                    }
                    v.go_back(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|v, _, _, cx| {
                    if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                        return;
                    }
                    v.go_forward(cx);
                }),
            )
            .on_action(cx.listener(|v, action: &ActivateNumberedTab, _, cx| {
                if v.options.is_none() && v.dialog.is_none() && !v.tabs.is_empty() {
                    let index = action.index;
                    if index < v.tabs.len() {
                        v.switch_tab(index, cx);
                    }
                }
            }))
            .on_action(cx.listener(|v, action: &ChangeNumberedView, w, cx| {
                if v.options.is_none() && v.dialog.is_none() && action.index < ViewMode::ALL.len() {
                    let requested = ViewMode::ALL[action.index].0;
                    v.view = requested;
                    v.settings.view = v.view;
                    if v.settings.remember_views {
                        v.settings.folder_views.insert(v.dir(), v.view);
                    }
                    v.content_scroll.set_offset(point(px(0.), px(0.)));
                    v.invalidate_list();
                    v.save_settings(cx);
                    w.focus(&v.focus);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|v, _: &ToggleFocused, w, cx| {
                if v.options.is_some()
                    || v.dialog.is_some()
                    || v.popup.is_some()
                    || !v.focus.is_focused(w)
                {
                    return;
                }
                let visible = v.visible_file_indices();
                let cursor = v
                    .keyboard_cursor
                    .as_ref()
                    .or(v.focus_item.as_ref())
                    .and_then(|path| v.entries.iter().position(|entry| &entry.path == path))
                    .filter(|index| visible.contains(index))
                    .or_else(|| {
                        v.selected
                            .iter()
                            .copied()
                            .find(|index| visible.contains(index))
                    })
                    .or_else(|| v.visible_file_order().first().copied());
                if let Some(index) = cursor {
                    v.select(index, true, false, w, cx);
                    v.keyboard_cursor = Some(v.entries[index].path.clone());
                }
            }))
            .on_action(cx.listener(|v, _: &SelectFocused, w, cx| {
                if v.options.is_some()
                    || v.dialog.is_some()
                    || v.popup.is_some()
                    || !v.focus.is_focused(w)
                {
                    return;
                }
                let visible = v.visible_file_indices();
                let cursor = v
                    .keyboard_cursor
                    .as_ref()
                    .or(v.focus_item.as_ref())
                    .and_then(|path| v.entries.iter().position(|entry| &entry.path == path))
                    .filter(|index| visible.contains(index))
                    .or_else(|| v.visible_file_order().first().copied());
                if let Some(index) = cursor {
                    v.select(index, false, false, w, cx);
                    v.keyboard_cursor = Some(v.entries[index].path.clone());
                }
            }))
            .on_action(cx.listener(|v, _: &ToggleDetails, w, cx| {
                if v.options.is_none() && v.dialog.is_none() {
                    v.settings.details_pane = !v.settings.details_pane;
                    if v.settings.details_pane {
                        v.preview_open = false;
                        v.settings.preview = false;
                    }
                    v.clear_preview();
                    v.save_settings(cx);
                    w.focus(&v.focus);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|v, _: &CycleFocus, w, cx| {
                if v.options.is_some() || v.dialog.is_some() {
                    return;
                }
                v.keyboard_focus_region = (v.keyboard_focus_region + 1) % 4;
                match v.keyboard_focus_region {
                    1 => v.focus_address(w, cx),
                    2 => {
                        v.editing_address = false;
                        v.search
                            .update(cx, |input, cx| input.focus_select_all(w, cx));
                    }
                    3 => v.focus_navigation(w, cx),
                    _ => {
                        v.editing_address = false;
                        w.focus(&v.focus);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &CycleFocusBack, w, cx| {
                if v.options.is_some() || v.dialog.is_some() {
                    return;
                }
                v.keyboard_focus_region = (v.keyboard_focus_region + 3) % 4;
                match v.keyboard_focus_region {
                    1 => v.focus_address(w, cx),
                    2 => {
                        v.editing_address = false;
                        v.search
                            .update(cx, |input, cx| input.focus_select_all(w, cx));
                    }
                    3 => v.focus_navigation(w, cx),
                    _ => {
                        v.editing_address = false;
                        w.focus(&v.focus);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &FullScreen, w, _| {
                if v.options.is_none() && v.dialog.is_none() {
                    w.toggle_fullscreen();
                }
            }))
            .on_action(cx.listener(|v, _: &ExpandCurrentTree, _, cx| {
                if v.options.is_none() && v.dialog.is_none() {
                    v.computer_open = true;
                    v.expand_to_current(cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|v, _: &Back, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.go_back(cx);
            }))
            .on_action(cx.listener(|v, _: &Forward, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.go_forward(cx);
            }))
            .on_action(cx.listener(|v, _: &Up, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.go_up(cx);
            }))
            .on_action(cx.listener(|v, _: &ZoomIn, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.change_zoom(0.1, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &ZoomOut, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.change_zoom(-0.1, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &ZoomReset, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.change_zoom(0., w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Refresh, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.reload(cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Address, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.focus_address(w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Search, w, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.search.update(cx, |v, cx| v.focus_select_all(w, cx));
            }))
            .on_action(cx.listener(|v, _: &Open, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    if v.focus.is_focused(w) {
                        v.open_selected_with(w.modifiers(), cx);
                    } else if v.nav_focus.is_focused(w) {
                        v.navigate_tree(rust_explorer::nav_keyboard::Command::Activate, w, cx);
                    }
                }
            }))
            .on_action(cx.listener(|v, _: &Undo, window, cx| {
                if v.focus.is_focused(window) || v.nav_focus.is_focused(window) {
                    v.replay_history(false, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Redo, window, cx| {
                if v.focus.is_focused(window) || v.nav_focus.is_focused(window) {
                    v.replay_history(true, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Copy, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.copy(false, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Cut, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.copy(true, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Paste, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.paste(cx);
                }
            }))
            .on_action(cx.listener(|v, _: &PermanentDelete, w, cx| {
                if v.options.is_none()
                    && v.dialog.is_none()
                    && v.popup.is_none()
                    && v.focus.is_focused(w)
                    && !v.busy
                {
                    let paths = v.paths();
                    if !paths.is_empty() {
                        v.show_dialog(Dialog::PermanentDelete(paths), w, cx);
                    }
                }
            }))
            .on_action(cx.listener(|v, _: &Delete, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.delete_dialog(w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Rename, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.rename_dialog(w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &NewFolder, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.show_dialog(Dialog::NewFolder, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &SelectAll, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.selected = v.visible_file_indices();
                v.clear_preview();
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &Next, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.key_move(1, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Previous, w, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.key_move(-1, w, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &TogglePreview, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.preview_open = !v.preview_open;
                v.load_preview(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &NewTab, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.new_tab(cx);
                }
            }))
            .on_action(cx.listener(|v, _: &NewWindow, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                let _ = open_explorer(
                    Some(Tab {
                        id: next_tab_id(),
                        history: History::new(v.dir()),
                        query: String::new(),
                        global: false,
                        reveal_targets: Vec::new(),
                    }),
                    cx,
                );
            }))
            .on_action(cx.listener(|v, _: &NextTab, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.switch_tab((v.active + 1) % v.tabs.len(), cx)
            }))
            .on_action(cx.listener(|v, _: &PreviousTab, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                v.switch_tab((v.active + v.tabs.len() - 1) % v.tabs.len(), cx)
            }))
            .on_action(cx.listener(|v, _: &CloseTab, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.close_tab(v.active, cx);
                }
            }))
            .on_action(cx.listener(|v, _: &ContextMenu, _, cx| {
                if v.options.is_none() && (v.dialog.is_none() || v.inline_rename.is_some()) {
                    v.menu(cx);
                }
            }))
            .on_action(cx.listener(|v, _: &Properties, _, cx| {
                if v.options.is_some() || (v.dialog.is_some() && v.inline_rename.is_none()) {
                    return;
                }
                let mut paths = v.paths();
                if paths.is_empty() {
                    if navigation::is_virtual(&v.dir()) {
                        v.run_shell(shell::Request::SystemProperties, cx);
                        return;
                    }
                    paths.push(v.dir());
                }
                v.run_shell(shell::Request::PropertiesMany { paths }, cx);
            }))
            .on_action(cx.listener(|v, _: &Escape, w, cx| {
                v.marquee = None;
                if v.popup.take().is_some() {
                    v.submenu = None;
                    v.nested_submenu = None;
                    v.pending_right_drop = None;
                    cx.notify();
                    return;
                }
                v.file_drag = None;
                v.right_file_drag = None;
                v.pending_right_drop = None;
                v.pending_selection_click = None;
                if let Some(flag) = v.tab_drag.take() {
                    flag.store(true, Ordering::Relaxed);
                    cx.stop_active_drag(w);
                } else if cx.has_active_drag() {
                    cx.stop_active_drag(w);
                }
                v.popup = None;
                v.submenu = None;
                v.options = None;
                v.rename_token += 1;
                if v.dialog.is_some() {
                    v.close_dialog(w, cx);
                } else {
                    v.selected.clear();
                    v.clear_preview();
                    cx.notify();
                }
            }));

        root = root
            .capture_any_mouse_down(cx.listener(|v, e: &MouseDownEvent, _, _| {
                v.marquee = None;
                v.pending_selection_click = None;
                v.right_file_drag = None;
                v.file_drag = None;
                if e.button == MouseButton::Left {
                    v.columns_resize = None;
                    v.volume_columns_resize = None;
                    v.sidebar_resize = None;
                    v.preview_resize = None;
                    v.aux_scroll_drag = None;
                    v.aux_horizontal_drag = None;
                    if v.scroll_drag.take().is_some() {
                        v.list_scroll.scrollbar_drag_ended();
                    }
                }
            }))
            .on_drag_move(cx.listener(|v, event: &DragMoveEvent<DragFiles>, _, cx| {
                if let Some(drag) = event.dragged_item().downcast_ref::<DragFiles>() {
                    v.update_file_drag(
                        drag.paths.clone(),
                        event.event.position,
                        event.event.modifiers,
                        cx,
                    );
                }
            }))
            .on_mouse_move({
                let listener = cx.listener(|v, event: &MouseMoveEvent, w, cx| {
                    if event.pressed_button == Some(MouseButton::Left) && v.marquee.is_some() {
                        v.update_marquee(event.position, cx);
                    }
                    if event.pressed_button == Some(MouseButton::Right) {
                        let right = v
                            .right_file_drag
                            .as_ref()
                            .map(|r| (r.index, r.paths.clone(), r.start, r.active));
                        if let Some((index, paths, start, active)) = right {
                            if active || (event.position - start).magnitude() > RIGHT_DRAG_SLOP {
                                if !active {
                                    if !v.selected.contains(&index) {
                                        v.select(index, false, false, w, cx);
                                    }
                                    if let Some(right) = &mut v.right_file_drag {
                                        right.active = true;
                                    }
                                }
                                v.update_file_drag(paths, event.position, event.modifiers, cx);
                            }
                        }
                    }
                    if let Some((start, width)) = v.preview_resize {
                        if event.dragging() {
                            v.settings.preview_width = (width
                                - (f32::from(event.position.x) - start) / v.zoom)
                                .clamp(180., 900.);
                            cx.notify();
                        }
                    }
                    if let Some((handle, start, offset, ratio)) = &v.aux_scroll_drag {
                        let mut p = handle.offset();
                        p.y = px((*offset - (f32::from(event.position.y) - *start) * *ratio)
                            .clamp(-f32::from(handle.max_offset().height), 0.));
                        handle.set_offset(p);
                        cx.notify();
                    }
                    if let Some((handle, start, offset, ratio)) = &v.aux_horizontal_drag {
                        let mut p = handle.offset();
                        p.x = px((*offset - (f32::from(event.position.x) - *start) * *ratio)
                            .clamp(-f32::from(handle.max_offset().width), 0.));
                        handle.set_offset(p);
                        cx.notify();
                    }
                    if let Some((horizontal, start, offset, ratio)) = v.scroll_drag {
                        let delta = if horizontal {
                            f32::from(event.position.x)
                        } else {
                            f32::from(event.position.y)
                        } - start;
                        if horizontal {
                            let mut point = v.content_scroll.offset();
                            point.x = px(offset - delta * ratio);
                            v.content_scroll.set_offset(point);
                        } else if v.high_performance_active() {
                            v.set_high_performance_row(offset + delta * ratio);
                        } else {
                            v.set_list_scroll_row(offset + delta * ratio);
                        }
                        cx.notify();
                    }
                    if let Some((start, width)) = v.sidebar_resize {
                        if event.dragging() {
                            v.settings.sidebar_width = (width
                                + (f32::from(event.position.x) - start) / v.zoom)
                                .clamp(140., 600.);
                            cx.notify();
                        }
                    }
                    if let Some((index, start, width)) = v.volume_columns_resize {
                        if event.dragging() {
                            if let Some((_, column_width)) =
                                v.settings.volume_columns.get_mut(index)
                            {
                                *column_width = (width
                                    + (f32::from(event.position.x) - start) / v.zoom)
                                    .clamp(64., 1200.);
                            }
                            cx.notify();
                        }
                    }
                    if let Some((index, start, width)) = v.columns_resize {
                        if event.dragging() {
                            v.active_columns_mut()[index].1 = (width
                                + (f32::from(event.position.x) - start) / v.zoom)
                                .clamp(64., 1200.);
                            v.invalidate_list();
                            cx.notify();
                        }
                    }
                });
                // Passive movement needs only GPUI's hover hit testing. Avoid
                // updating the entire Explorer entity for every raw mouse event.
                move |event: &MouseMoveEvent, window, cx: &mut App| {
                    if event.pressed_button.is_some() {
                        listener(event, window, cx);
                    }
                }
            })
            .capture_any_mouse_up(cx.listener(|v, e: &MouseUpEvent, w, cx| {
                if e.button == MouseButton::Left {
                    if let Some(drag) = v.marquee.take() {
                        if drag.active {
                            v.focus_item = v
                                .selected
                                .first()
                                .and_then(|i| v.entries.get(*i))
                                .map(|e| e.path.clone());
                            v.load_preview(cx);
                        }
                        cx.notify();
                    }
                }
                if e.button == MouseButton::Right {
                    if let Some(right) = v.right_file_drag.take() {
                        if right.active {
                            v.update_file_drag(right.paths.clone(), e.position, e.modifiers, cx);
                            if let Some(target) =
                                v.file_drag.as_ref().and_then(|d| d.target.clone())
                            {
                                v.pending_right_drop = Some((right.paths, target));
                                v.popup = Some(("drop-menu".into(), e.position));
                                v.submenu = None;
                            }
                            v.file_drag = None;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }
                    return;
                }
                if e.button != MouseButton::Left {
                    return;
                }
                if let Some(paths) = v.file_drag.as_ref().map(|d| d.paths.clone()) {
                    v.update_file_drag(paths, e.position, e.modifiers, cx);
                }
                if let Some(index) = v.pending_selection_click.take() {
                    if !cx.has_active_drag() && (v.selected.len() > 1 || e.modifiers.control) {
                        v.select(index, e.modifiers.control, e.modifiers.shift, w, cx);
                    }
                }
                // Drop listeners still need the modifier-selected operation.
                cx.defer_in(w, |v, _, cx| {
                    v.file_drag = None;
                    cx.notify();
                });
                if v.volume_columns_resize.take().is_some() {
                    v.save_settings(cx);
                }
                if v.columns_resize.take().is_some() {
                    v.save_settings(cx);
                }
                if v.preview_resize.take().is_some() {
                    v.save_settings(cx);
                }
                v.aux_scroll_drag = None;
                v.aux_horizontal_drag = None;
                if v.scroll_drag.take().is_some() {
                    v.list_scroll.scrollbar_drag_ended();
                    cx.notify();
                }
                if v.sidebar_resize.take().is_some() {
                    v.save_settings(cx);
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    let changed = v.volume_columns_resize.take().is_some()
                        | v.columns_resize.take().is_some()
                        | v.sidebar_resize.take().is_some()
                        | v.preview_resize.take().is_some();
                    v.aux_scroll_drag = None;
                    v.aux_horizontal_drag = None;
                    if v.scroll_drag.take().is_some() {
                        v.list_scroll.scrollbar_drag_ended();
                        cx.notify();
                    }
                    if changed {
                        v.save_settings(cx);
                    }
                }),
            );
        root = root.child(self.render_tabs(window, cx));

        // The navigation bar uses the active tab's fill color; the shorter row
        // halves the vertical gap above and below the 32 px fields.
        let mut nav = div()
            .h(ui(rust_explorer::layout_metrics::ADDRESS_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(t.input));
        for (id, label) in [
            ("back", "←"),
            ("forward", "→"),
            ("up", "↑"),
            ("refresh", "⟳"),
        ] {
            // Unavailable navigation is greyed out and does nothing when clicked.
            let enabled = match id {
                "back" => self.tabs[self.active].history.can_back(),
                "forward" => self.tabs[self.active].history.can_forward(),
                "up" => self.can_go_up(),
                _ => true,
            };
            let click = cx.listener(move |v, _, _, cx| match id {
                "back" => v.go_back(cx),
                "forward" => v.go_forward(cx),
                "up" => v.go_up(cx),
                _ => v.reload(cx),
            });
            nav = nav.child(if id == "refresh" {
                // Match the other navigation buttons while retaining a centered glyph.
                div()
                    .id(id)
                    .w(ui(30.))
                    .h(ui(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(ui(4.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(t.hover)))
                    .child(visual::icon_sized("refresh", t.text, 16.))
                    .on_click(click)
            } else {
                button(id, label, t)
                    .when(!enabled, |d| d.opacity(0.35))
                    .on_click(click)
            });
        }
        let address_width = (logical_width - 492.).max(80.);
        let address = if self.editing_address {
            let bounds = self.address_bounds.clone();
            div()
                .relative()
                .flex_1()
                .relative()
                .flex()
                .items_center()
                .h(ui(32.))
                .rounded(ui(4.))
                .bg(rgb(t.panel))
                .text_color(rgb(t.text))
                // A little left padding so the caret is not glued to the border when
                // the address bar is in edit mode.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl(ui(6.))
                        .child(self.address.clone()),
                )
                .child(
                    div()
                        .id("clear-address")
                        .w(ui(28.))
                        .h(ui(28.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(visual::icon_sized("close", t.text, 12.))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|v, _, w, cx| {
                                cx.stop_propagation();
                                v.address_candidates.clear();
                                v.address_choice = None;
                                v.address_generation = v.address_generation.wrapping_add(1);
                                let value = v.address.read(cx).value();
                                if value == v.address_before_edit || value.is_empty() {
                                    v.address.update(cx, |input, cx| {
                                        input.set_value(v.address_before_edit.clone(), cx)
                                    });
                                    v.editing_address = false;
                                    w.focus(&v.focus);
                                    cx.notify();
                                    return;
                                }
                                v.address.update(cx, |input, cx| {
                                    input.set_value(String::new(), cx);
                                    input.focus_end(w, cx);
                                });
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    canvas(move |rect, _, _| bounds.set(rect), |_, _, _, _| {})
                        .absolute()
                        .inset_0(),
                )
                .into_any_element()
        } else {
            self.render_breadcrumbs(address_width, window, cx)
        };
        self.sync_search_placeholder(cx);
        nav = nav.child(address).child(
            div()
                .w(ui(260.))
                .h(ui(32.))
                .flex()
                .items_center()
                .px_2()
                .gap_2()
                .bg(rgb(t.panel))
                .rounded(ui(4.))
                .child(icon("\u{e721}", t.muted))
                .child(self.search.clone()),
        );
        root = root.child(nav);

        let mut toolbar = div()
            .id("commands")
            .overflow_hidden()
            .flex_shrink_0()
            .h(ui(48.))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(rgb(t.border));
        for (id, label) in [
            ("new-folder", "⊕ 新建"),
            ("cut", "剪切"),
            ("copy", "复制"),
            ("paste", "粘贴"),
            ("rename", "重命名"),
            ("share", "共享"),
            ("delete", "删除"),
        ] {
            if self.toolbar_overflow.contains(&id) {
                continue;
            }
            let command = if id == "new-folder" {
                button(id, label, t).child(visual::icon("chevron-down", t.muted))
            } else {
                button(id, "", t).child(visual::icon(id, t.text))
            };
            let enabled = match id {
                "new-folder" => !navigation::is_virtual(&self.dir()),
                "paste" => self.can_paste(),
                "rename" => {
                    let drive = self
                        .selected
                        .first()
                        .and_then(|index| self.entries.get(*index))
                        .is_some_and(|entry| volume_label::is_drive_root(&entry.path));
                    self.selected.len() == 1
                        && (drive || (!self.is_home() && !navigation::is_shell_folder(&self.dir())))
                }
                "share" => {
                    self.selected.len() == 1
                        && !navigation::is_shell_folder(&self.dir())
                        && self
                            .selected
                            .first()
                            .is_some_and(|i| !self.entries[*i].directory)
                }
                _ => !self.selected.is_empty(),
            };
            toolbar = toolbar.child(
                command
                    .w(ui(if id == "new-folder" { 94. } else { 40. }))
                    .flex_shrink_0()
                    .when(!enabled, |d| d.opacity(0.4).cursor_default())
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if !enabled {
                            return;
                        }
                        match id {
                            "new-folder" => {
                                v.popup = Some((
                                    "new-menu".into(),
                                    point(px(12. * v.zoom), px(132. * v.zoom)),
                                ));
                                cx.notify();
                            }
                            "cut" => v.copy(true, cx),
                            "copy" => v.copy(false, cx),
                            "paste" => v.paste(cx),
                            "rename" => v.rename_dialog(w, cx),
                            "share" => {
                                if v.selected.len() == 1
                                    && v.selected.first().is_some_and(|i| !v.entries[*i].directory)
                                {
                                    v.run_shell(shell::Request::Share { paths: v.paths() }, cx);
                                }
                            }
                            _ => {
                                if id == "delete" && w.modifiers().shift {
                                    let paths = v.paths();
                                    if !paths.is_empty() {
                                        v.show_dialog(Dialog::PermanentDelete(paths), w, cx);
                                    }
                                } else {
                                    v.delete_dialog(w, cx);
                                }
                            }
                        }
                    })),
            );
            if id == "new-folder" {
                toolbar = toolbar.child(div().w(ui(1.)).h(ui(24.)).mx_2().bg(rgb(t.border)));
            }
        }
        toolbar = toolbar.child(div().w(ui(1.)).h(ui(20.)).mx_2().bg(rgb(t.border)));
        for (id, label) in [
            ("sort-menu", "排序"),
            ("view-menu", "查看"),
            ("more-menu", ""),
        ] {
            if self.toolbar_overflow.contains(&id) {
                continue;
            }
            if id == "more-menu" {
                toolbar = toolbar.child(div().w(ui(1.)).h(ui(24.)).mx_2().bg(rgb(t.border)));
            }
            toolbar = toolbar.child(
                button(id, "", t)
                    .w(ui(if id == "more-menu" { 40. } else { 96. }))
                    .flex_shrink_0()
                    .child(visual::icon(
                        match id {
                            "sort-menu" => "sort",
                            "view-menu" => "view",
                            _ => "more",
                        },
                        t.text,
                    ))
                    .when(!label.is_empty(), |d| {
                        d.child(label).child(visual::icon("chevron-down", t.muted))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, event: &MouseDownEvent, _, cx| {
                            v.popup = Some((
                                id.into(),
                                point(event.position.x, event.position.y + px(23. * v.zoom)),
                            ));
                            cx.notify();
                        }),
                    ),
            );
        }
        if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
            toolbar = toolbar.child(
                button("empty-recycle-bin", "清空回收站", t)
                    .w(ui(120.))
                    .flex_shrink_0()
                    .when(self.busy, |d| d.opacity(0.4).cursor_default())
                    .on_click(cx.listener(|v, _, _, cx| {
                        if !v.busy {
                            v.run_shell(shell::Request::EmptyRecycleBin, cx);
                        }
                    })),
            );
        }
        toolbar = toolbar
            .child(
                button(
                    "usage",
                    if self.usage_running {
                        "停止统计"
                    } else {
                        "统计大小"
                    },
                    t,
                )
                .when(
                    self.view != ViewMode::Details
                        || navigation::is_virtual(&self.dir())
                        || self.toolbar_overflow.contains(&"usage"),
                    |d| d.hidden(),
                )
                .w(ui(104.))
                .flex_shrink_0()
                .on_click(cx.listener(|v, _, _, cx| v.start_usage(cx))),
            )
            .child(
                button(
                    "global",
                    if self.tabs[self.active].global {
                        "全盘搜索"
                    } else {
                        "文件夹内搜索"
                    },
                    t,
                )
                .w(ui(120.))
                .flex_shrink_0()
                .when(self.toolbar_overflow.contains(&"global"), |d| d.hidden())
                .on_click(cx.listener(|v, _, _, cx| {
                    v.tabs[v.active].global = !v.tabs[v.active].global;
                    v.sync_search_placeholder(cx);
                    if !v.tabs[v.active].query.is_empty() {
                        v.reload(cx);
                    }
                    cx.notify();
                })),
            )
            .child(div().flex_1())
            .child(
                button("preview-toggle", "预览", t)
                    .w(ui(72.))
                    .flex_shrink_0()
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.preview_open = !v.preview_open;
                        v.settings.preview = v.preview_open;
                        v.save_settings(cx);
                        v.load_preview(cx);
                        cx.notify();
                    })),
            );
        root = root.child(toolbar);

        let sidebar = self.render_sidebar(sidebar_width, cx);

        let mut content = div()
            .id("file-content")
            .overflow_hidden()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .relative();
        if self.is_home() || (self.is_device_root() && !self.storage_volumes.is_empty()) {
            content = content
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(self.render_computer(window, cx))
                        .child(self.auxiliary_scrollbar(
                            self.home_scroll.clone(),
                            "home-scrollbar",
                            cx,
                        )),
                )
                .when(
                    matches!(self.view, ViewMode::Details | ViewMode::List),
                    |d| {
                        d.child(self.auxiliary_horizontal_scrollbar(
                            self.home_scroll.clone(),
                            "home-horizontal-scrollbar",
                            cx,
                        ))
                    },
                );
        } else {
            let table_width = if self.view == ViewMode::Details {
                self.column_widths(available)
                    .iter()
                    .map(|(_, w)| *w)
                    .sum::<f32>()
            } else if self.view == ViewMode::List {
                self.layout_width.max(
                    self.rows
                        .iter()
                        .filter_map(|r| {
                            if let DisplayRow::List(i) = r {
                                Some(i.len() as f32 * 230.)
                            } else {
                                None
                            }
                        })
                        .fold(0., f32::max),
                )
            } else {
                available
            };
            let high_performance = self.high_performance_active();
            let row_height = self.high_performance_row_height();
            let header_height = if self.view == ViewMode::Details {
                34.
            } else {
                0.
            };
            let viewport_height = (list_height - header_height).max(row_height);
            let visible_rows = (viewport_height / row_height).floor().max(1.) as usize;
            self.high_performance_visible.set(visible_rows);
            if high_performance {
                let current = self.high_performance_row.get();
                self.high_performance_row
                    .set(current.clamp(0., self.high_performance_max_row()));
            }
            let mut table = div()
                .id("table-viewport")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col();
            let body: AnyElement;
            if high_performance {
                let first_row = self.high_performance_row.get().floor() as usize;
                let mut scroller = div()
                    .id("high-performance-scroll")
                    .overflow_x_scroll()
                    .track_scroll(&self.content_scroll)
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col();
                scroller.style().restrict_scroll_to_axis = Some(true);
                if self.view == ViewMode::Details {
                    scroller = scroller.child(self.render_header(available, cx));
                }
                let mut pool = div()
                    .id("high-performance-pool")
                    .relative()
                    .w(ui(table_width.max(available)))
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                        if e.modifiers.control || e.modifiers.shift {
                            return;
                        }
                        let delta = e.delta.pixel_delta(px(40.));
                        let amount = if f32::from(delta.y).abs() > 0.1 {
                            f32::from(delta.y)
                        } else {
                            f32::from(delta.x)
                        };
                        if amount != 0. {
                            v.scroll_high_performance_by(-amount);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }));
                for slot in 0..visible_rows + 2 {
                    let row = first_row + slot;
                    if row >= self.rows.len() {
                        break;
                    }
                    pool = pool.child(
                        div()
                            .id(("high-performance-slot", slot))
                            .absolute()
                            .top(ui(slot as f32 * row_height))
                            .left_0()
                            .w_full()
                            .h(ui(row_height))
                            .overflow_hidden()
                            .child({
                                let retained =
                                    self.recycled_rows[slot % self.recycled_rows.len()].clone();
                                retained.update(cx, |state, _| {
                                    state.index = row;
                                });
                                retained.into_any_element()
                            }),
                    );
                }
                body = scroller.child(pool).into_any_element();
            } else {
                table = table.overflow_x_scroll().track_scroll(&self.content_scroll);
                table.style().restrict_scroll_to_axis = Some(true);
                if self.view == ViewMode::Details {
                    table = table.child(self.render_header(available, cx));
                }
                body = list(
                    self.list_scroll.clone(),
                    cx.processor(move |this, index: usize, window, cx| {
                        if this.recycled_rows.is_empty() {
                            this.render_list_row(index, window, cx)
                        } else {
                            let slot = this.recycled_rows[index % this.recycled_rows.len()].clone();
                            slot.update(cx, |row, _| {
                                row.index = index;
                            });
                            slot.into_any_element()
                        }
                    }),
                )
                .flex_1()
                .min_h_0()
                .into_any_element();
            }
            table = table.child(
                div()
                    .id("list-area")
                    .w(ui(if high_performance {
                        available
                    } else {
                        table_width.max(available)
                    }))
                    .flex_shrink_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, e: &MouseDownEvent, w, cx| {
                            if v.inline_rename.is_some() {
                                v.confirm_dialog(w, cx);
                                if v.inline_rename.is_some() {
                                    return;
                                }
                            }
                            // Double-clicking the empty area goes to the parent
                            // folder. Item clicks stop propagation, so reaching this
                            // handler means the pointer is not on an item.
                            if e.click_count >= 2 {
                                v.marquee = None;
                                v.go_up(cx);
                                return;
                            }
                            v.begin_marquee(e, w, cx);
                        }),
                    )
                    .on_drop(cx.listener(|v, drag: &ExternalPaths, _, cx| {
                        v.confirm_drop(drag.paths().to_vec(), v.dir(), false, cx);
                    }))
                    .on_drop(cx.listener(|v, drag: &DragFiles, w, cx| {
                        v.confirm_drop(drag.paths.clone(), v.dir(), w.modifiers().control, cx);
                    }))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|v, e: &MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            v.selected.clear();
                            v.clear_preview();
                            w.focus(&v.focus);
                            v.menu(cx);
                            if let Some((_, p)) = &mut v.popup {
                                *p = e.position;
                            }
                        }),
                    )
                    .relative()
                    .child(body)
                    .child(self.list_wheel_guard(cx)),
            );
            content =
                content
                    .child(table)
                    .child(self.horizontal_scrollbar(available, table_width, cx));
            content = if high_performance {
                content.child(self.high_performance_vertical_scrollbar(
                    viewport_height,
                    header_height,
                    cx,
                ))
            } else {
                content.child(self.vertical_scrollbar(cx))
            };
            if self.entries.is_empty() {
                content = content.child(
                    div()
                        .absolute()
                        .top(ui(85.))
                        .left_0()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_color(rgb(t.muted))
                        .child(if self.loading {
                            "正在加载…"
                        } else if self.error.is_some() {
                            "无法读取此位置"
                        } else {
                            "此文件夹为空，或没有匹配的结果"
                        }),
                );
            }
        }
        let mut body = div()
            // GPUI's automatic focus handler prevents the default Win32
            // non-client action. Keep it below the native caption hit regions.
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .flex()
            .child(sidebar)
            .child(
                div()
                    .id("resize-sidebar")
                    .w(ui(5.))
                    .h_full()
                    .flex_shrink_0()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(move |s| s.bg(rgb(t.accent)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.sidebar_resize = Some((e.position.x.into(), sidebar_width));
                        }),
                    ),
            )
            .child(content);

        if self.preview_open || self.settings.details_pane {
            body = body.child(
                div()
                    .id("resize-preview")
                    .w(ui(5.))
                    .h_full()
                    .flex_shrink_0()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(move |s| s.bg(rgb(t.accent)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.preview_resize = Some((e.position.x.into(), preview_width));
                        }),
                    ),
            );
            let title = self
                .selected
                .first()
                .and_then(|i| self.entries.get(*i))
                .map(|v| v.name.clone())
                .unwrap_or("预览".into());
            let mut pane = div()
                .w(ui(preview_width))
                .flex_shrink_0()
                .h_full()
                .border_l_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.panel))
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_h(ui(34.))
                        .py_2()
                        .flex_shrink_0()
                        .px_4()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .border_b_1()
                        .border_color(rgb(t.border))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .whitespace_normal()
                                .line_height(ui(20.))
                                .child(title.clone()),
                        ),
                );
            let element = match &self.preview {
                Preview::Empty => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(rgb(t.muted))
                    .child(icon("\u{e8a5}", t.muted))
                    .child(if self.selected.len() > 1 {
                        format!("已选择 {} 个项目", self.selected.len())
                    } else {
                        "选择文件以预览".into()
                    })
                    .into_any_element(),
                Preview::Loading => div()
                    .p_5()
                    .text_color(rgb(t.muted))
                    .child("正在准备预览…")
                    .into_any_element(),
                Preview::Message(message) => div()
                    .p_5()
                    .text_color(rgb(t.muted))
                    .child(message.clone())
                    .into_any_element(),
                Preview::Image(path) | Preview::Video(path) => div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(path.clone())
                            .image_cache(&self.thumbnail_images)
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element(),
                Preview::Text(lines, truncated, encoding) => {
                    let lines = lines.clone();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .text_color(rgb(t.muted))
                                .text_size(ui(11.))
                                .child(*encoding),
                        )
                        .when(*truncated, |d| {
                            d.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_color(rgb(t.accent))
                                    .child("预览已截断（最多 256 KB / 6000 行）"),
                            )
                        })
                        .child(
                            uniform_list("text-preview", lines.len(), move |range, _, _| {
                                range
                                    .map(|i| {
                                        div()
                                            .h(ui(22.))
                                            .px_3()
                                            .font_family("Consolas")
                                            .text_size(ui(12.))
                                            .overflow_hidden()
                                            .child(lines[i].clone())
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .flex_1()
                            .min_h_0(),
                        )
                        .into_any_element()
                }
                Preview::Pdf { path, page, pages } => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(ui(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .child(
                                button("pdf-prev", "←", t)
                                    .on_click(cx.listener(|v, _, _, cx| v.pdf_step(-1, cx))),
                            )
                            .child(format!("{} / {}", page + 1, pages))
                            .child(
                                button("pdf-next", "→", t)
                                    .on_click(cx.listener(|v, _, _, cx| v.pdf_step(1, cx))),
                            ),
                    )
                    .child(
                        div().flex_1().min_h_0().p_3().child(
                            img(path.clone())
                                .image_cache(&self.thumbnail_images)
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                    )
                    .into_any_element(),
            };
            let element =
                if self.settings.details_pane || matches!(self.preview, Preview::Message(_)) {
                    let selected: Vec<_> = self
                        .selected
                        .iter()
                        .filter_map(|i| self.entries.get(*i))
                        .collect();
                    let mut data = div()
                        .id("selected-entry-details")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .text_color(rgb(t.muted));
                    if selected.len() == 1 {
                        let entry = selected[0];
                        data = data
                            .child(visual::entry_icon(
                                &entry.path,
                                entry.directory,
                                entry.link,
                                t.text,
                                96.,
                            ))
                            .child(
                                div()
                                    .text_color(rgb(t.text))
                                    .whitespace_normal()
                                    .child(entry.name.clone()),
                            )
                            .child(format!("类型：{}", entry.kind))
                            .child(format!("大小：{}", self.entry_size_text(entry)))
                            .child(format!("修改日期：{}", entry.date()))
                            .child(format!("创建日期：{}", extras::format_date(entry.created)))
                            .child(
                                div()
                                    .whitespace_normal()
                                    .child(entry.path.display().to_string()),
                            );
                    } else {
                        data = data
                            .child(format!("已选择 {} 个项目", selected.len()))
                            .child(format!(
                                "文件：{} 个",
                                selected.iter().filter(|e| !e.directory).count()
                            ))
                            .child(format!(
                                "文件夹：{} 个",
                                selected.iter().filter(|e| e.directory).count()
                            ));
                    }
                    data.into_any_element()
                } else {
                    element
                };
            pane = pane.child(element);
            body = body.child(pane);
        }
        root = root.child(body);
        let status = if self.busy
            || self
                .system_operations
                .iter()
                .any(|op| op.finished.is_none())
        {
            "正在处理系统操作…".to_owned()
        } else if self.loading {
            "正在加载…".to_owned()
        } else {
            format!(
                "{}{}",
                if self.selected.is_empty() {
                    self.status.clone()
                } else {
                    format!("{} 个项目", self.entries.len())
                },
                if self.selected.is_empty() {
                    String::new()
                } else {
                    let bytes = self
                        .selected
                        .iter()
                        .filter_map(|i| self.entries.get(*i))
                        .fold(0u64, |sum, entry| {
                            sum.saturating_add(if !entry.directory {
                                entry.size
                            } else if self.usage_enabled {
                                self.usage.get(&entry.path).map_or(0, |u| u.bytes)
                            } else {
                                0
                            })
                        });
                    format!(
                        " | 已选择 {} 项 | {} {}",
                        self.selected.len(),
                        if self.usage_enabled {
                            "统计大小"
                        } else {
                            "文件大小"
                        },
                        fs::size_label(bytes)
                    )
                }
            )
        };
        root = root.child(
            div()
                .h(ui(32.))
                .flex_shrink_0()
                .px_4()
                .flex()
                .items_center()
                .justify_between()
                .border_t_1()
                .border_color(rgb(t.border))
                .bg(rgb(t.chrome))
                .text_size(ui(12.))
                .text_color(rgb(t.muted))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("system-operation-details")
                                .cursor_pointer()
                                .child(status)
                                .on_click(cx.listener(|v, _, w, cx| {
                                    v.popup =
                                        Some(("system-operations".into(), w.mouse_position()));
                                    cx.notify();
                                })),
                        )
                        .when(
                            self.system_operations
                                .iter()
                                .any(|op| op.finished.is_none()),
                            |d| {
                                d.child(button("system-operation-stop", "停止", t).on_click(
                                    cx.listener(|v, _, _, cx| {
                                        for op in v
                                            .system_operations
                                            .iter_mut()
                                            .filter(|op| op.finished.is_none())
                                        {
                                            op.control.stop(false);
                                            op.status = "正在请求停止".into();
                                        }
                                        cx.notify();
                                    }),
                                ))
                            },
                        ),
                )
                .child(format!(
                    "{:.0}% · Ctrl +/− 缩放 · Ctrl+0 重置",
                    self.zoom * 100.
                )),
        );
        if self.editing_address && !self.address_candidates.is_empty() {
            let bounds = self.address_bounds.get();
            let t = self.theme;
            let mut suggestions = div()
                .id("address-completions")
                .w_full()
                .h(ui(self.address_candidates.len().min(9) as f32 * 26.))
                .pr_3()
                .overflow_y_scroll()
                .track_scroll(&self.address_scroll)
                .bg(rgb(t.input))
                .border_1()
                .border_color(rgb(t.border))
                .shadow_md();
            for (index, candidate) in self.address_candidates.iter().enumerate() {
                let path = candidate.path.clone();
                suggestions = suggestions.child(
                    div()
                        .id(("path-candidate", index))
                        .h(ui(26.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .when(self.address_choice == Some(index), |d| {
                            d.bg(rgb(t.selection))
                        })
                        .hover(move |d| d.bg(rgb(t.hover)))
                        .child(visual::file_icon(
                            &candidate.path,
                            candidate.directory,
                            t.text,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(
                                    candidate
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, _, window, cx| {
                                cx.stop_propagation();
                                v.open_address_path(path.clone(), window, cx);
                            }),
                        ),
                );
            }
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .left(bounds.origin.x)
                        .top(bounds.bottom())
                        .w(bounds.size.width)
                        .child(suggestions)
                        .child(self.auxiliary_scrollbar(
                            self.address_scroll.clone(),
                            "address-scrollbar",
                            cx,
                        )),
                )
                .with_priority(2),
            );
        }
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .absolute()
                    .bottom(ui(44.))
                    .left(ui(220.))
                    .right(ui(25.))
                    .rounded(ui(6.))
                    .p_3()
                    .bg(rgb(if t.dark { 0x493030 } else { 0xffe5e5 }))
                    .border_1()
                    .border_color(rgb(0x9b5555))
                    .flex()
                    .gap_3()
                    .child(div().flex_1().child(error.clone()))
                    .child(
                        button("dismiss-error", "×", t).on_click(cx.listener(|v, _, _, cx| {
                            v.error = None;
                            cx.notify();
                        })),
                    ),
            );
        }
        if let Some(dialog) = &self.dialog {
            if self.inline_rename.is_none() {
                let (title, description, delete) = match dialog {
                    Dialog::NewFolder => ("新建文件夹", "输入新文件夹的名称".to_owned(), false),
                    Dialog::Shortcut => (
                        "创建快捷方式",
                        "输入目标文件或文件夹的完整路径".to_owned(),
                        false,
                    ),
                    Dialog::Rename(_) => ("重命名", "输入新的名称".to_owned(), false),
                    Dialog::RenameComputer { .. } => {
                        ("重命名此电脑", "输入新的名称".to_owned(), false)
                    }
                    Dialog::RenameVolume { path, .. } => (
                        "重命名驱动器",
                        format!(
                            "为 {} 输入卷标；确认后保存，留空可清除卷标。",
                            path.display()
                        ),
                        false,
                    ),
                    Dialog::Transfer {
                        paths,
                        destination,
                        copy_only,
                        mode,
                    } => (
                        if *mode == shell::TransferMode::Link {
                            "确认创建快捷方式"
                        } else if *mode == shell::TransferMode::Move {
                            "确认移动"
                        } else if *copy_only || *mode == shell::TransferMode::Copy {
                            "确认复制"
                        } else {
                            "确认拖放操作"
                        },
                        format!(
                            "将 {} 个项目放入：{}\n{}",
                            paths.len(),
                            destination.display(),
                            if *mode == shell::TransferMode::Link {
                                "在目标文件夹创建这些项目的快捷方式。"
                            } else if *mode == shell::TransferMode::Move {
                                "将项目移动到目标文件夹。"
                            } else if *copy_only || *mode == shell::TransferMode::Copy {
                                "复制到目标文件夹。"
                            } else {
                                "同一文件系统内移动，不同文件系统间复制；无法识别的卷按复制处理。"
                            }
                        ),
                        true,
                    ),
                    Dialog::Delete(paths) => (
                        "删除文件",
                        match paths.as_slice() {
                            [path] => format!("是否删除 {}？", item_name(path, &self.entries)),
                            _ => format!(
                                "是否删除这 {} 个项目？\n{}",
                                paths.len(),
                                item_names(&paths, 3, &self.entries)
                            ),
                        },
                        true,
                    ),
                    Dialog::PermanentDelete(paths) => (
                        "永久删除 — 无法撤销",
                        match paths.as_slice() {
                            [path] => format!(
                                "是否删除 {}？\n不会移至回收站，无法通过本程序撤销。",
                                item_name(path, &self.entries)
                            ),
                            _ => format!(
                                "是否删除这 {} 个项目？\n不会移至回收站，无法通过本程序撤销。\n\n{}",
                                paths.len(),
                                item_names(&paths, 3, &self.entries)
                            ),
                        },
                        true,
                    ),
                };
                root = root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .bg(rgba(0x00000070))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .w(ui(440.))
                                .rounded(ui(10.))
                                .p_6()
                                .bg(rgb(t.panel))
                                .border_1()
                                .border_color(rgb(t.border))
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(
                                    div()
                                        .text_size(ui(19.))
                                        .when(
                                            matches!(dialog, Dialog::PermanentDelete(_)),
                                            |element| element.text_color(rgb(0xe24b4b)),
                                        )
                                        .child(title),
                                )
                                .child(description)
                                .when(!delete, |d| {
                                    d.child(
                                        div()
                                            .bg(rgb(t.input))
                                            .rounded(ui(4.))
                                            .p_1()
                                            .child(self.dialog_input.clone()),
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(button("cancel-dialog", "取消", t).on_click(
                                            cx.listener(|v, _, w, cx| v.close_dialog(w, cx)),
                                        ))
                                        .child(
                                            button(
                                                "confirm-dialog",
                                                if matches!(dialog, Dialog::PermanentDelete(_)) {
                                                    "永久删除"
                                                } else if matches!(dialog, Dialog::Delete(_)) {
                                                    "移至回收站"
                                                } else {
                                                    "确定"
                                                },
                                                t,
                                            )
                                            .bg(rgb(t.selection))
                                            .when(
                                                matches!(dialog, Dialog::PermanentDelete(_)),
                                                |element| {
                                                    element
                                                        .bg(rgb(0xa62929))
                                                        .text_color(rgb(0xffffff))
                                                },
                                            )
                                            .on_click(
                                                cx.listener(|v, _, w, cx| v.confirm_dialog(w, cx)),
                                            ),
                                        ),
                                ),
                        ),
                );
            }
        }
        if self.popup.is_some() {
            root = root.child(self.render_popup(window, cx));
        }
        if let Some(drag) = self.marquee.as_ref().filter(|drag| drag.active) {
            root = root.child(
                div()
                    .absolute()
                    .left(drag.origin.x.min(drag.current.x))
                    .top(drag.origin.y.min(drag.current.y))
                    .w((drag.current.x - drag.origin.x).abs())
                    .h((drag.current.y - drag.origin.y).abs())
                    .border_1()
                    .border_color(rgb(t.accent))
                    .bg(rgba(if t.dark { 0x76b9ed30 } else { 0x0078d430 })),
            );
        }
        if self.options.is_some() {
            root = root.child(self.render_options(cx));
        }

        if let Some(drag) = &self.file_drag {
            root = root.child(
                div()
                    .absolute()
                    .left(drag.position.x + px(16. * self.zoom))
                    .top(drag.position.y + px(20. * self.zoom))
                    .max_w(ui(460.))
                    .px_3()
                    .py_1()
                    .rounded(ui(4.))
                    .shadow_md()
                    .border_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.panel))
                    .text_color(rgb(t.text))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.file_drag_label(drag)),
            );
        }

        root
    }
}

pub fn run(launch: Option<rust_explorer::launch_request::LaunchRequest>) {
    rust_explorer::fluent_icons::initialize();
    // Context menus are now built in-process, so there is no helper to pre-warm;
    // the Shell's handlers are loaded together with the application's own COM.
    Application::new()
        .with_assets(rust_explorer::fluent_icons::Assets)
        .run(move |cx: &mut App| {
            input::init(cx);
            cx.bind_keys([
                KeyBinding::new("alt-left", Back, Some("Explorer")),
                KeyBinding::new("alt-right", Forward, Some("Explorer")),
                KeyBinding::new("alt-up", Up, Some("Explorer")),
                KeyBinding::new("ctrl-+", ZoomIn, None),
                KeyBinding::new("ctrl-=", ZoomIn, None),
                KeyBinding::new("ctrl-shift-=", ZoomIn, None),
                KeyBinding::new("ctrl--", ZoomOut, None),
                KeyBinding::new("ctrl-0", ZoomReset, None),
                KeyBinding::new("f5", Refresh, Some("Explorer")),
                KeyBinding::new("ctrl-l", Address, Some("Explorer")),
                KeyBinding::new("ctrl-f", Search, Some("Explorer")),
                KeyBinding::new("enter", Open, Some("Explorer")),
                KeyBinding::new("ctrl-enter", Open, Some("Explorer")),
                KeyBinding::new("shift-enter", Open, Some("Explorer")),
                KeyBinding::new("ctrl-shift-enter", Open, Some("Explorer")),
                KeyBinding::new("ctrl-c", Copy, Some("Explorer")),
                KeyBinding::new("ctrl-x", Cut, Some("Explorer")),
                KeyBinding::new("ctrl-v", Paste, Some("Explorer")),
                KeyBinding::new("ctrl-z", Undo, Some("Explorer")),
                KeyBinding::new("ctrl-y", Redo, Some("Explorer")),
                KeyBinding::new("delete", Delete, Some("Explorer")),
                KeyBinding::new("shift-delete", PermanentDelete, Some("Explorer")),
                KeyBinding::new("f2", Rename, Some("Explorer")),
                KeyBinding::new("ctrl-shift-n", NewFolder, Some("Explorer")),
                KeyBinding::new("ctrl-a", SelectAll, Some("Explorer")),
                KeyBinding::new("down", Next, Some("Explorer")),
                KeyBinding::new("up", Previous, Some("Explorer")),
                KeyBinding::new("alt-p", TogglePreview, Some("Explorer")),
                KeyBinding::new("ctrl-t", NewTab, Some("Explorer")),
                KeyBinding::new("ctrl-n", NewWindow, Some("Explorer")),
                KeyBinding::new("ctrl-tab", NextTab, None),
                KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
                KeyBinding::new("ctrl-w", CloseTab, Some("Explorer")),
                KeyBinding::new("escape", Escape, Some("Explorer")),
                KeyBinding::new("shift-f10", ContextMenu, Some("Explorer")),
                KeyBinding::new("alt-enter", Properties, Some("Explorer")),
                KeyBinding::new("alt-d", Address, Some("Explorer")),
                KeyBinding::new("f4", Address, Some("Explorer")),
                KeyBinding::new("ctrl-e", Search, Some("Explorer")),
                KeyBinding::new("f3", Search, Some("Explorer")),
                KeyBinding::new("ctrl-r", Refresh, Some("Explorer")),
                KeyBinding::new("backspace", Back, Some("Explorer")),
                KeyBinding::new("ctrl-d", Delete, Some("Explorer")),
                KeyBinding::new("ctrl-f4", CloseTab, Some("Explorer")),
                KeyBinding::new("alt-shift-p", ToggleDetails, Some("Explorer")),
                KeyBinding::new("ctrl-space", ToggleFocused, Some("Explorer")),
                KeyBinding::new("space", SelectFocused, Some("Explorer")),
                KeyBinding::new("f6", CycleFocus, Some("Explorer")),
                KeyBinding::new("shift-f6", CycleFocusBack, Some("Explorer")),
                KeyBinding::new("f11", FullScreen, Some("Explorer")),
                KeyBinding::new("ctrl-shift-e", ExpandCurrentTree, Some("Explorer")),
            ]);
            for (key, command) in [
                ("up", rust_explorer::nav_keyboard::Command::Up),
                ("down", rust_explorer::nav_keyboard::Command::Down),
                ("home", rust_explorer::nav_keyboard::Command::First),
                ("end", rust_explorer::nav_keyboard::Command::Last),
                ("left", rust_explorer::nav_keyboard::Command::Left),
                ("right", rust_explorer::nav_keyboard::Command::Right),
                ("+", rust_explorer::nav_keyboard::Command::Expand),
                ("-", rust_explorer::nav_keyboard::Command::Collapse),
                ("*", rust_explorer::nav_keyboard::Command::ExpandLevel),
                ("shift-8", rust_explorer::nav_keyboard::Command::ExpandLevel),
                ("enter", rust_explorer::nav_keyboard::Command::Activate),
                ("space", rust_explorer::nav_keyboard::Command::Activate),
                (
                    "shift-f10",
                    rust_explorer::nav_keyboard::Command::ContextMenu,
                ),
                ("menu", rust_explorer::nav_keyboard::Command::ContextMenu),
            ] {
                cx.bind_keys([KeyBinding::new(
                    key,
                    NavigateTree { command },
                    Some("NavigationTree"),
                )]);
            }
            for index in 0..9 {
                cx.bind_keys([KeyBinding::new(
                    &format!("ctrl-{}", index + 1),
                    ActivateNumberedTab { index },
                    Some("Explorer"),
                )]);
            }
            for index in 0..8 {
                cx.bind_keys([KeyBinding::new(
                    &format!("ctrl-shift-{}", index + 1),
                    ChangeNumberedView { index },
                    Some("Explorer"),
                )]);
                // Windows GPUI translates Shift+digits to punctuation before
                // matching; retain digit bindings for other keyboard layouts.
                cx.bind_keys([KeyBinding::new(
                    &format!("ctrl-{}", ["!", "@", "#", "$", "%", "^", "&", "*"][index]),
                    ChangeNumberedView { index },
                    Some("Explorer"),
                )]);
            }
            for (key, step) in [
                ("left", NavigationStep::Horizontal(-1)),
                ("right", NavigationStep::Horizontal(1)),
                ("up", NavigationStep::Vertical(-1)),
                ("down", NavigationStep::Vertical(1)),
                ("home", NavigationStep::First),
                ("end", NavigationStep::Last),
                ("pageup", NavigationStep::Page(-1)),
                ("pagedown", NavigationStep::Page(1)),
            ] {
                // Later bindings replace the original Up/Down aliases with the
                // same navigation semantics plus explicit modifier selection.
                cx.bind_keys([KeyBinding::new(
                    key,
                    NavigateFiles {
                        step: step.clone(),
                        extend: false,
                        focus_only: false,
                    },
                    Some("Explorer"),
                )]);
                cx.bind_keys([KeyBinding::new(
                    &format!("shift-{key}"),
                    NavigateFiles {
                        step: step.clone(),
                        extend: true,
                        focus_only: false,
                    },
                    Some("Explorer"),
                )]);
                cx.bind_keys([KeyBinding::new(
                    &format!("ctrl-{key}"),
                    NavigateFiles {
                        step: step.clone(),
                        extend: false,
                        focus_only: true,
                    },
                    Some("Explorer"),
                )]);
            }
            open_explorer_request(None, launch, cx).expect("无法创建窗口");
            cx.activate(true);
        });
}
