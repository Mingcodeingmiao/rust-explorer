use super::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use unicode_segmentation::UnicodeSegmentation;

thread_local! {static TAB_WINDOWS:RefCell<HashMap<isize,WeakEntity<Explorer>>>=RefCell::new(HashMap::new());}
pub(super) struct TabDrop {
    pub id: u64,
    pub accepted: bool,
    pub position: (i32, i32),
}
pub(super) fn native_hwnd(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn set_application_icon(handle: isize) {
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
        },
        core::PCWSTR,
    };
    unsafe {
        let hwnd = HWND(handle as *mut _);
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (kind, logical_size) in [(ICON_SMALL, 16u32), (ICON_BIG, 32u32)] {
            let size = ((logical_size * dpi + 48) / 96) as i32;
            if let Ok(icon) = LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(101usize as *const u16),
                IMAGE_ICON,
                size,
                size,
                LR_SHARED,
            ) {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
}

static TAB_ID: AtomicU64 = AtomicU64::new(1);
pub(super) fn next_tab_id() -> u64 {
    TAB_ID.fetch_add(1, Ordering::Relaxed)
}
#[derive(Clone)]
pub(super) enum DisplayRow {
    Group {
        label: String,
        count: usize,
        collapsed: bool,
    },
    Items(Vec<usize>),
    List(Vec<ListCell>),
}
#[derive(Clone)]
struct DragColumn {
    id: String,
}
struct NameTip {
    hint: rust_explorer::file_hints::FileHint,
    visible: bool,
    theme: Theme,
    tip: Option<rust_explorer::file_tip::FileTip>,
    tip_scale: f32,
}
impl NameTip {
    fn new(entry: Entry, theme: Theme, cx: &mut Context<Self>) -> Self {
        use rust_explorer::file_hints::{FileHint, FileHintRequest, HintLine};
        let mut lines = Vec::new();
        let mut push = |label: &str, value: String| {
            if !value.is_empty() {
                lines.push(HintLine {
                    label: label.into(),
                    value,
                });
            }
        };
        if entry.directory {
            push("创建日期", format_date(entry.created));
            push("大小", "未统计".into());
        } else {
            push("项目类型", entry.kind.clone());
            push("大小", fs::size_label(entry.size));
            push("修改日期", entry.date());
        }
        let hint = FileHint {
            name: entry.name,
            lines,
            partial: entry.directory,
        };
        // GPUI creates tooltip views after its own 500 ms hover delay. Add
        // another 500 ms without holding the view alive; a departed pointer
        // drops this view and its weak update fails before any helper is run.
        cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            if view
                .update(cx, |tip, cx| {
                    tip.visible = true;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let request = FileHintRequest { path: entry.path };
            let task = cx.background_executor().spawn(async move {
                let json = serde_json::to_string(&request).ok()?;
                volumes::bounded::<FileHint>("--file-hint", &json, Duration::from_millis(1200)).ok()
            });
            if let Some(hint) = task.await {
                let _ = view.update(cx, |tip, cx| {
                    tip.hint = hint;
                    cx.notify();
                });
            }
        })
        .detach();
        Self {
            hint,
            visible: false,
            theme,
            tip: None,
            tip_scale: 1.0,
        }
    }
}
impl Drop for NameTip {
    fn drop(&mut self) {
        if let Some(tip) = self.tip.as_mut() {
            tip.hide();
        }
    }
}
impl Render for NameTip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        // GPUI pixels are logical: the native card needs the monitor DPI as well as
        // the user's zoom, otherwise it renders at 100% size on a scaled display.
        let scale = (f32::from(window.rem_size()) / 16.) * window.scale_factor();
        if shell::menu_active() || !window.is_window_active() {
            // The card belongs to this window: a native menu, deactivation or another
            // application taking the foreground must hide it rather than leave it on
            // screen.
            if let Some(tip) = self.tip.as_mut() {
                tip.hide();
            }
            return div().into_any_element();
        }
        if self.tip.is_none() || (self.tip_scale - scale).abs() > 0.01 {
            let owner = shell::active_window_owner();
            let owner = (owner != 0).then_some(owner as isize);
            self.tip = rust_explorer::file_tip::FileTip::new(owner, scale, self.theme.dark);
            self.tip_scale = scale;
        }
        let text = std::iter::once(self.hint.name.clone())
            .chain(
                self.hint
                    .lines
                    .iter()
                    .map(|line| format!("{}：{}", line.label, line.value)),
            )
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(tip) = self.tip.as_mut() {
            // Explorer shows the hover card just below-right of the pointer. The
            // card is its own click-through window, so it can neither sit under the
            // pointer nor race the mouse-up that opens the context menu.
            tip.show(&text, shell::cursor());
        }
        div().into_any_element()
    }
}
impl Render for DragColumn {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .bg(rgb(0x333d49))
            .text_color(rgb(0xffffff))
            .child(column_label(&self.id, false))
    }
}
#[derive(Clone)]
struct DragTab {
    tab: Tab,
    source: WeakEntity<Explorer>,
    accepted: Arc<AtomicBool>,
}
struct TabGhost {
    tab: Tab,
    name: String,
    accepted: Arc<AtomicBool>,
    width: f32,
    theme: Theme,
    sender: mpsc::Sender<TabDrop>,
}
impl Drop for TabGhost {
    fn drop(&mut self) {
        let _ = self.sender.send(TabDrop {
            id: self.tab.id,
            accepted: self.accepted.load(Ordering::Relaxed),
            position: shell::cursor(),
        });
    }
}
impl Render for TabGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(ui(self.width))
            .h(ui(32.4))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .rounded_t(ui(7.))
            .bg(rgb(self.theme.input))
            .text_color(rgb(self.theme.text))
            .text_size(ui(13. * 0.9))
            .child(visual::icon("folder", self.theme.text))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.name.clone()),
            )
            .child(visual::icon_at("close", self.theme.text, 16. * 2. / 3.))
    }
}

pub(super) fn open_explorer(
    tab: Option<Tab>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Explorer>> {
    open_explorer_request(tab, None, cx)
}
pub(super) fn open_explorer_request(
    tab: Option<Tab>,
    launch: Option<rust_explorer::launch_request::LaunchRequest>,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<Explorer>> {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let external = launch.is_some();
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.), px(400.))),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("Rust Explorer {}", env!("CARGO_PKG_VERSION")).into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        move |w, cx| {
            let view = cx.new(|cx| Explorer::new(w, launch, cx));
            if let Some(hwnd) = native_hwnd(w) {
                set_application_icon(hwnd);
                if let Err(error) = rust_explorer::window_frame::install(hwnd) {
                    eprintln!("{error:#}");
                } else {
                    rust_explorer::instance::advertise(hwnd);
                }
                TAB_WINDOWS.with(|windows| {
                    windows.borrow_mut().insert(hwnd, view.downgrade());
                });
            }
            view.update(cx, |v, cx| {
                if let Some(tab) = tab {
                    v.external_reveal = tab.reveal_targets.clone();
                    v.reveal_pending = !v.external_reveal.is_empty();
                    v.tabs = vec![tab];
                    v.active = 0;
                    v.search
                        .update(cx, |input, cx| input.set_value(v.tabs[0].query.clone(), cx));
                    // The window was built for its default folder; resolve the
                    // presentation for the folder it now shows instead of keeping
                    // the previous one (e.g. This PC's tiles + type grouping).
                    let dir = v.dir();
                    v.apply_listing_defaults(&dir);
                }
                v.reload(cx);
            });
            view
        },
    )?;
    if external {
        // GPUI applies initial visibility after the constructor closure returns.
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| {
                if let Some(hwnd) = native_hwnd(window) {
                    rust_explorer::instance::reveal_window(hwnd);
                }
            });
        });
    }
    Ok(handle)
}
pub(super) fn compare_entries(
    a: &Entry,
    b: &Entry,
    column: Sort,
    descending: bool,
) -> std::cmp::Ordering {
    if column == Sort::Modified && a.directory != b.directory {
        return a.directory.cmp(&b.directory);
    }
    let value = match column {
        Sort::Name => fs::natural_cmp(&a.name, &b.name),
        Sort::Modified => a.modified.cmp(&b.modified),
        Sort::Created => a.created.cmp(&b.created),
        Sort::Original => a
            .recycle
            .as_ref()
            .map(|i| &i.original_location)
            .cmp(&b.recycle.as_ref().map(|i| &i.original_location)),
        Sort::Deleted => a
            .recycle
            .as_ref()
            .and_then(|i| i.deleted)
            .cmp(&b.recycle.as_ref().and_then(|i| i.deleted)),
        Sort::Kind => a.kind.cmp(&b.kind),
        Sort::Size => a.size.cmp(&b.size),
    }
    .then_with(|| fs::natural_cmp(&a.name, &b.name));
    if descending { value.reverse() } else { value }
}
fn is_image_entry(entry: &Entry) -> bool {
    !entry.directory && matches!(rust_explorer::preview::kind(&entry.path), Kind::Image)
}
fn column_sort(id: &str) -> Sort {
    match id {
        "original" => Sort::Original,
        "deleted" => Sort::Deleted,
        "modified" => Sort::Modified,
        "created" => Sort::Created,
        "kind" => Sort::Kind,
        "size" => Sort::Size,
        _ => Sort::Name,
    }
}
fn column_label(id: &str, usage: bool) -> &'static str {
    match id {
        "original" => "原位置",
        "deleted" => "删除日期",
        "modified" => "修改日期",
        "created" => "创建日期",
        "kind" => "类型",
        "size" if usage => "统计大小",
        "size" => "大小",
        _ => "名称",
    }
}
pub(super) fn icon_label_lines(
    text: &str,
    width: f32,
    font_size: f32,
    scale: f32,
    window: &Window,
) -> Vec<String> {
    rust_explorer::icon_layout::label_lines(text.graphemes(true), width.max(1.), 4, |line| {
        let run = TextRun {
            len: line.len(),
            font: font("Microsoft YaHei UI"),
            color: rgb(0).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(
            window
                .text_system()
                .shape_line(line.to_owned().into(), px(font_size * scale), &[run], None)
                .width,
        ) / scale
    })
}
pub(super) fn icon_label(
    text: &str,
    width: f32,
    font_size: f32,
    scale: f32,
    window: &Window,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .line_height(ui(20.))
        .text_size(ui(font_size))
        .children(
            icon_label_lines(text, width, font_size, scale, window)
                .into_iter()
                .map(|line| {
                    div()
                        .h(ui(20.))
                        .w_full()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(line)
                }),
        )
        .into_any_element()
}
fn menu_icon(id: &str) -> Option<&'static str> {
    Some(if id.starts_with("view:") {
        match id {
            "view:0" => "extra-large-icons",
            "view:1" => "large-icons",
            "view:2" => "medium-icons",
            "view:3" => "small-icons",
            "view:4" => "list",
            "view:5" => "details",
            "view:6" => "tiles",
            "view:7" => "content",
            _ => "view-grid",
        }
    } else if id.starts_with("template:") {
        "file"
    } else {
        match id {
            "new-folder" => "folder",
            "new-shortcut" => "shortcut",
            "zip" => "zip",
            "file-cut" => "cut",
            "file-copy" => "copy",
            "file-paste" => "paste",
            "file-rename" => "rename",
            "file-delete" => "delete",
            "file-share" => "share",
            "submenu:view-menu" => "view",
            "submenu:sort-menu" => "sort",
            "pin" | "nav-pin" | "nav-unpin" => "pin",
            "copy-path" | "nav-copy-path" => "copy-path",
            "select-all" => "select-all",
            "select-none" => "select-none",
            "select-invert" => "invert-selection",
            "checkboxes" => "checkbox",
            "properties" | "nav-properties" => "properties",
            "options" => "options",
            "preview-pane" | "details-pane" => "preview",
            "extensions" => "file",
            "hidden" => "eye",
            _ => return None,
        }
    })
}

impl Explorer {
    fn change_quick_access(&mut self, paths: Vec<PathBuf>, pinned: bool, cx: &mut Context<Self>) {
        if paths.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let folder = path.is_dir();
                    let result = if folder {
                        shell::execute_output(shell::Request::QuickAccessPin {
                            path: path.clone(),
                            pinned,
                        })
                        .map(|_| ())
                    } else {
                        Ok(())
                    };
                    (path, folder, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |view, cx| {
            let results = task.await;
            let _ = view.update(cx, |v, cx| {
                v.busy = false;
                let mut errors = Vec::new();
                let mut local_only = false;
                for (path, folder, result) in results {
                    // Local overlays remain useful for files (not supported by
                    // Windows pin-to-home) and as an explicit fallback on failure.
                    let fallback = result.is_err() || !folder;
                    if let Err(error) = result {
                        errors.push(format!("{}：{error}", path.display()));
                    }
                    v.settings.favorites.retain(|p| p != &path);
                    v.settings.file_favorites.retain(|p| p != &path);
                    v.settings.quick_access_hidden.retain(|p| p != &path);
                    if pinned && fallback {
                        v.settings.favorites.push(path.clone());
                        if !folder {
                            v.settings.file_favorites.push(path.clone());
                        }
                    } else if !pinned && fallback {
                        v.settings.quick_access_hidden.push(path.clone());
                    }
                    local_only |= fallback;
                }
                v.status = if local_only {
                    "已更新本程序快速访问；部分项目未写入系统固定列表".into()
                } else {
                    "已更新 Windows 快速访问".into()
                };
                if !errors.is_empty() {
                    v.error = Some(format!(
                        "系统快速访问更新失败，已仅更新本程序列表：\n{}",
                        errors.join("\n")
                    ));
                }
                v.quick_access_tick = Instant::now() - Duration::from_secs(16);
                v.refresh_quick_access(cx);
                if v.dir() == PathBuf::from("::QuickAccess") {
                    v.sync_computer_entries();
                }
                v.invalidate_list();
                v.save_settings(cx);
                cx.notify();
            });
        })
        .detach();
    }
    /// Queue the folder's per-item icons (executables, shortcuts, drives) once, so a
    /// scroll does not have to wait for each real icon.
    pub(super) fn prefetch_entry_icons(&self) {
        if self.is_home() && self.volumes.is_empty() {
            return;
        }
        let stamp = (self.generation, visual::icon_pixels());
        if self.icon_prefetch.get() == Some(stamp) {
            return;
        }
        self.icon_prefetch.set(Some(stamp));
        let pixels = visual::icon_pixels();
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| {
                rust_explorer::native_assets::IconRequest::needs_own_icon(
                    &entry.path,
                    entry.directory,
                )
            })
            .map(|entry| {
                rust_explorer::native_assets::IconRequest::file(
                    &entry.path,
                    entry.directory,
                    pixels,
                )
            })
            .take(rust_explorer::native_assets::ICON_PREFETCH_LIMIT)
            .collect();
        visual::prefetch_icons(keys);
    }
    pub(super) fn sync_thumbnails(&self) {
        let enabled = matches!(
            self.view,
            ViewMode::Medium | ViewMode::Large | ViewMode::ExtraLarge | ViewMode::Content
        ) && self.settings.thumbnails
            && !self.is_home()
            && !self.loading;
        let pixels = if enabled {
            visual::thumbnail_pixels(self.view.icon_size())
        } else {
            0
        };
        // Eager whole-folder prefetch. It must not sit behind the viewport stamp:
        // the first render of a folder usually hits that early return, which used to
        // leave the queue with only the visible rows ("a page per scroll").
        let prefetch_stamp = (self.generation, pixels);
        if enabled {
            if !self.entries.is_empty() && self.thumbnail_prefetch.get() != Some(prefetch_stamp) {
                self.thumbnail_prefetch.set(Some(prefetch_stamp));
                let all: Vec<_> = self
                    .entries
                    .iter()
                    .filter_map(|entry| rust_explorer::thumbnails::Key::from_entry(entry, pixels))
                    .take(rust_explorer::thumbnails::MAX_PREFETCH)
                    .collect();
                self.thumbnails.set_all(all);
            }
        } else if self.thumbnail_prefetch.get().is_some() {
            // No previews are shown: drop the queue and stop generating.
            self.thumbnail_prefetch.set(None);
            self.thumbnails.set_all(Vec::new());
        }
        let first = if enabled {
            self.list_scroll
                .logical_scroll_top()
                .item_ix
                .saturating_sub(1)
        } else {
            0
        };
        let end = if enabled {
            (first + (self.layout_height / self.estimated_row_height()).ceil() as usize + 3)
                .min(self.rows.len())
        } else {
            0
        };
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 30;
        let stamp = (self.generation, first, end, pixels, epoch);
        if self.thumbnail_viewport.get() == Some(stamp) {
            return;
        }
        self.thumbnail_viewport.set(Some(stamp));
        let keys = self
            .rows
            .get(first..end)
            .unwrap_or_default()
            .iter()
            .filter_map(|row| {
                if let DisplayRow::Items(items) = row {
                    Some(items)
                } else {
                    None
                }
            })
            .flatten()
            .filter_map(|index| self.entries.get(*index))
            .filter_map(|entry| rust_explorer::thumbnails::Key::from_entry(entry, pixels))
            .take(rust_explorer::thumbnails::MAX_VISIBLE)
            .collect();
        self.thumbnails.set_visible(keys);
    }
    pub(super) fn estimated_row_height(&self) -> f32 {
        match self.view {
            ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium => {
                self.view.icon_size() + 56.
            }
            ViewMode::Tiles | ViewMode::Content => 74.,
            _ if self.settings.compact => rust_explorer::layout_metrics::COMPACT_FILE_ROW,
            _ => 34.,
        }
    }
    /// High-performance mode keeps a fixed pool of row widgets and only
    /// changes their data. Grouped lists use variable-height headers, so they
    /// fall back to the regular measured virtual list.
    pub(super) fn high_performance_active(&self) -> bool {
        self.settings.high_performance
            && self.group.is_empty()
            && !self.is_home()
            && matches!(
                self.view,
                ViewMode::Details | ViewMode::List | ViewMode::Small | ViewMode::Tiles
            )
    }
    pub(super) fn high_performance_row_height(&self) -> f32 {
        self.estimated_row_height().max(1.)
    }
    pub(super) fn high_performance_max_row(&self) -> f32 {
        let visible = self.high_performance_visible.get().max(1);
        self.rows.len().saturating_sub(visible) as f32
    }
    pub(super) fn set_high_performance_row(&self, row: f32) {
        self.high_performance_row
            .set(row.clamp(0., self.high_performance_max_row()));
    }
    pub(super) fn scroll_high_performance_by(&self, pixels: f32) {
        let row_height = self.high_performance_row_height() * self.zoom;
        let current = self.high_performance_row.get();
        self.set_high_performance_row(current + pixels / row_height.max(1.));
    }
    pub(super) fn set_list_scroll_row(&self, row: f32) {
        let height = self.estimated_row_height() * self.zoom;
        let visible = f32::from(self.list_scroll.viewport_bounds().size.height) / height;
        let row = row.clamp(0., (self.rows.len() as f32 - visible).max(0.));
        self.list_scroll.scroll_to(ListOffset {
            item_ix: row.floor() as usize,
            offset_in_item: px(row.fract() * height),
        });
    }
    pub(super) fn list_wheel_guard(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let viewport = self.content_scroll.clone();
        let blocked = self.options.is_some()
            || self.popup.is_some()
            || (self.dialog.is_some() && self.inline_rename.is_none());
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let weak = weak.clone();
                let viewport = viewport.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture
                        || blocked
                        || !bounds.contains(&e.position)
                        || !viewport.bounds().contains(&e.position)
                    {
                        return;
                    }
                    let _ = weak.update(cx, |v, cx| {
                        if v.high_performance_active() && !e.modifiers.control && !e.modifiers.shift
                        {
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
                                return;
                            }
                        }
                        if e.modifiers.control {
                            let delta = f32::from(e.delta.pixel_delta(px(40.)).y);
                            if v.view_wheel_remainder.signum() != delta.signum() {
                                v.view_wheel_remainder = 0.;
                            }
                            v.view_wheel_remainder += delta;
                            let steps = (v.view_wheel_remainder / 40.).trunc() as isize;
                            if steps != 0 {
                                v.view_wheel_remainder -= steps as f32 * 40.;
                                let levels = [
                                    ViewMode::Content,
                                    ViewMode::Tiles,
                                    ViewMode::Details,
                                    ViewMode::List,
                                    ViewMode::Small,
                                    ViewMode::Medium,
                                    ViewMode::Large,
                                    ViewMode::ExtraLarge,
                                ];
                                let current =
                                    levels.iter().position(|mode| *mode == v.view).unwrap_or(2);
                                let next = (current as isize + steps)
                                    .clamp(0, levels.len() as isize - 1)
                                    as usize;
                                if levels[next] != v.view {
                                    v.view = levels[next];
                                    v.settings.view = v.view;
                                    if v.settings.remember_views {
                                        v.settings.folder_views.insert(v.dir(), v.view);
                                    }
                                    v.content_scroll.set_offset(point(px(0.), px(0.)));
                                    v.list_scroll.scroll_to(ListOffset::default());
                                    v.invalidate_list();
                                    v.save_settings(cx);
                                }
                            }
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        if v.view == ViewMode::List || e.modifiers.shift {
                            let d = e.delta.pixel_delta(px(40.));
                            let mut p = v.content_scroll.offset();
                            p.x += if d.x != px(0.) { d.x } else { d.y };
                            v.content_scroll.set_offset(p);
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
    pub(super) fn auxiliary_scrollbar(
        &self,
        handle: ScrollHandle,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = f32::from(handle.bounds().size.height);
        let max = f32::from(handle.max_offset().height);
        if height <= 1. || max <= 1. {
            return div().into_any_element();
        }
        let thumb = (height * height / (height + max))
            .max(28. * self.zoom)
            .min(height);
        let travel = (height - thumb).max(1.);
        let offset = f32::from(handle.offset().y);
        let top = (-offset / max).clamp(0., 1.) * travel;
        let t = self.theme;
        div()
            .id(id)
            .absolute()
            .right_0()
            .top_0()
            .h(px(height))
            .w(ui(12.))
            .bg(rgb(t.panel))
            .child(
                div()
                    .id((id, 0usize))
                    .absolute()
                    .top(px(top))
                    .left(ui(3.))
                    .h(px(thumb))
                    .w(ui(6.))
                    .rounded(ui(3.))
                    .bg(rgb(t.muted))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.aux_scroll_drag =
                                Some((handle.clone(), e.position.y.into(), offset, max / travel));
                        }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn auxiliary_horizontal_scrollbar(
        &self,
        handle: ScrollHandle,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = f32::from(handle.bounds().size.width);
        let max = f32::from(handle.max_offset().width);
        // Occupy a footer outside the viewport so neither scroll bar obscures
        // the last drive row. Leave the lower-right corner to the vertical bar.
        let track = (viewport
            - if f32::from(handle.max_offset().height) > 1. {
                12. * self.zoom
            } else {
                0.
            })
        .max(1.);
        let t = self.theme;
        if viewport <= 1. || max <= 1. {
            return div().h(ui(14.)).w_full().flex_shrink_0().into_any_element();
        }
        let thumb = (track * viewport / (viewport + max))
            .max(28. * self.zoom)
            .min(track);
        let travel = (track - thumb).max(1.);
        let offset = f32::from(handle.offset().x);
        let left = (-offset / max).clamp(0., 1.) * travel;
        let track_handle = handle.clone();
        div()
            .id(id)
            .h(ui(14.))
            .w_full()
            .flex_shrink_0()
            .relative()
            .bg(rgb(t.panel))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let position = f32::from(e.position.x - track_handle.bounds().origin.x);
                    let fraction = ((position - thumb / 2.) / travel).clamp(0., 1.);
                    let mut p = track_handle.offset();
                    p.x = px(-fraction * max);
                    track_handle.set_offset(p);
                    v.aux_horizontal_drag = Some((
                        track_handle.clone(),
                        e.position.x.into(),
                        f32::from(p.x),
                        max / travel,
                    ));
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id((id, 0usize))
                    .absolute()
                    .left(px(left))
                    .top(ui(3.))
                    .w(px(thumb))
                    .h(ui(8.))
                    .rounded(ui(4.))
                    .bg(rgb(t.muted))
                    .hover(move |s| s.bg(rgb(t.text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.aux_horizontal_drag =
                                Some((handle.clone(), e.position.x.into(), offset, max / travel));
                        }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn confirm_drop(
        &mut self,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        copy_only: bool,
        cx: &mut Context<Self>,
    ) {
        let mode = if copy_only {
            shell::TransferMode::Copy
        } else {
            self.file_drag.as_ref().map(|d| d.mode).unwrap_or_default()
        };
        self.confirm_drop_mode(paths, destination, mode, cx);
    }
    fn confirm_drop_mode(
        &mut self,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        mode: shell::TransferMode,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty()
            || navigation::is_app_virtual(&destination)
            || self.busy
            || self.options.is_some()
            || self.dialog.is_some()
        {
            return;
        }
        self.rename_token += 1;
        self.popup = None;
        self.submenu = None;
        self.nested_submenu = None;
        self.dialog = Some(Dialog::Transfer {
            paths,
            destination,
            copy_only: mode == shell::TransferMode::Copy,
            mode,
        });
        cx.notify();
    }
    pub(super) fn update_file_drag(
        &mut self,
        paths: Vec<PathBuf>,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let hit = self
            .file_drop_bounds
            .borrow()
            .iter()
            .rev()
            .find(|(_, bounds, _)| bounds.contains(&position))
            .map(|(path, _, folder)| (path.clone(), *folder));
        let target = match hit {
            Some((path, true)) => Some(path),
            Some((_, false)) => None,
            None if self.content_scroll.bounds().contains(&position)
                && !navigation::is_app_virtual(&self.dir()) =>
            {
                Some(self.dir())
            }
            _ => None,
        }
        .filter(|target| {
            !paths
                .iter()
                .any(|source| target == source || target.starts_with(source))
        });
        let mode = if modifiers.alt {
            shell::TransferMode::Link
        } else if modifiers.control {
            shell::TransferMode::Copy
        } else if modifiers.shift {
            shell::TransferMode::Move
        } else {
            shell::TransferMode::Auto
        };
        self.file_drag = Some(FileDragState {
            paths,
            position,
            target,
            mode,
        });
        cx.notify();
    }
    pub(super) fn file_drag_label(&self, drag: &FileDragState) -> String {
        let Some(target) = &drag.target else {
            return format!("{} 个项目 · 不能放置到此处", drag.paths.len());
        };
        let operation = match drag.mode {
            shell::TransferMode::Copy => "复制到",
            shell::TransferMode::Move => "移动到",
            shell::TransferMode::Link => "创建快捷方式到",
            shell::TransferMode::Auto => {
                let target_volume = target
                    .components()
                    .next()
                    .map(|p| p.as_os_str().to_string_lossy().to_lowercase());
                if drag.paths.iter().all(|path| {
                    path.components()
                        .next()
                        .map(|p| p.as_os_str().to_string_lossy().to_lowercase())
                        == target_volume
                }) {
                    "移动到"
                } else {
                    "复制到"
                }
            }
        };
        format!(
            "{} 个项目 · {operation} {}",
            drag.paths.len(),
            target
                .file_name()
                .unwrap_or(target.as_os_str())
                .to_string_lossy()
        )
    }
    pub(super) fn horizontal_scrollbar(
        &self,
        available: f32,
        total: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if total <= available + 1. {
            return div().into_any_element();
        }
        let track = available * self.zoom;
        let max = (total - available) * self.zoom;
        let thumb = (track * available / total).max(28. * self.zoom).min(track);
        let travel = (track - thumb).max(1.);
        let offset = f32::from(self.content_scroll.offset().x);
        let left = (-offset / max).clamp(0., 1.) * travel;
        let t = self.theme;
        div()
            .id("horizontal-scrollbar")
            .h(ui(14.))
            .w_full()
            .flex_shrink_0()
            .relative()
            .bg(rgb(t.panel))
            .child(
                div()
                    .id("horizontal-thumb")
                    .absolute()
                    .left(px(left))
                    .top(ui(3.))
                    .h(ui(8.))
                    .w(px(thumb))
                    .rounded(ui(4.))
                    .bg(rgb(t.muted))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.scroll_drag = Some((true, e.position.x.into(), offset, max / travel));
                        }),
                    ),
            )
            .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                let d = e.delta.pixel_delta(px(40.));
                let mut p = v.content_scroll.offset();
                p.x += if d.x != px(0.) { d.x } else { d.y };
                v.content_scroll.set_offset(p);
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }
    pub(super) fn high_performance_vertical_scrollbar(
        &self,
        height: f32,
        top: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total = self.rows.len() as f32;
        let visible = self.high_performance_visible.get().max(1) as f32;
        let max = (total - visible).max(0.);
        if max <= 0.01 || height <= 1. || total <= 0. {
            return div().into_any_element();
        }
        let physical_height = height * self.zoom;
        let thumb = (physical_height * visible / total.max(1.))
            .max(28. * self.zoom)
            .min(physical_height);
        let travel = (physical_height - thumb).max(1.);
        let offset = self.high_performance_row.get().clamp(0., max);
        let thumb_top = offset / max * travel;
        let t = self.theme;
        div()
            .id("high-performance-scrollbar")
            .absolute()
            .right_0()
            .top(ui(top))
            .h(px(physical_height))
            .w(ui(12.))
            .bg(rgb(t.panel))
            .child(
                div()
                    .id("high-performance-thumb")
                    .absolute()
                    .top(px(thumb_top))
                    .left(ui(3.))
                    .w(ui(6.))
                    .h(px(thumb))
                    .rounded(ui(3.))
                    .bg(rgb(t.muted))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.scroll_drag =
                                Some((false, e.position.y.into(), offset, max / travel));
                        }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn vertical_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.view == ViewMode::List {
            return div().into_any_element();
        }
        let height = f32::from(self.list_scroll.viewport_bounds().size.height);
        let row_height = self.estimated_row_height() * self.zoom;
        let visible_rows = height / row_height;
        let max = (self.rows.len() as f32 - visible_rows).max(0.);
        if max <= 0.01 || height <= 1. {
            return div().into_any_element();
        }
        let thumb = (height * visible_rows / self.rows.len().max(1) as f32)
            .max(28. * self.zoom)
            .min(height);
        let travel = (height - thumb).max(1.);
        let logical = self.list_scroll.logical_scroll_top();
        let offset = logical.item_ix as f32 + f32::from(logical.offset_in_item) / row_height;
        let top = (offset / max).clamp(0., 1.) * travel;
        let t = self.theme;
        div()
            .id("vertical-scrollbar")
            .absolute()
            .right_0()
            .top(ui(if self.view == ViewMode::Details {
                34.
            } else {
                0.
            }))
            .w(ui(12.))
            .h(px(height))
            .bg(rgb(t.panel))
            .child(
                div()
                    .id("vertical-thumb")
                    .absolute()
                    .top(px(top))
                    .left(ui(3.))
                    .w(ui(6.))
                    .h(px(thumb))
                    .rounded(ui(3.))
                    .bg(rgb(t.muted))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.list_scroll.scrollbar_drag_started();
                            v.scroll_drag =
                                Some((false, e.position.y.into(), offset, max / travel));
                        }),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn render_breadcrumbs(
        &self,
        available: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let mut parts: Vec<(String, PathBuf)> = if navigation::is_shell_folder(&self.dir()) {
            let mut parts = vec![(navigation::computer_name(), volumes::computer())];
            if self
                .shell_chain
                .last()
                .is_some_and(|(_, path)| path == &self.dir())
            {
                parts.extend(self.shell_chain.iter().cloned());
            } else {
                let label = self
                    .shell_names
                    .get(&self.dir())
                    .cloned()
                    .unwrap_or_else(|| navigation::location_name(&self.dir()));
                parts.push((label, self.dir()));
            }
            parts
        } else if navigation::is_app_virtual(&self.dir()) {
            vec![(navigation::location_name(&self.dir()), self.dir())]
        } else {
            let mut parts = vec![(navigation::computer_name(), volumes::computer())];
            parts.extend(
                self.dir()
                    .ancestors()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|p| {
                        let label = self
                            .volumes
                            .iter()
                            .find(|v| v.path == p)
                            .map(|v| v.label.clone())
                            .unwrap_or_else(|| navigation::location_name(p));
                        (label, p.to_path_buf())
                    }),
            );
            parts
        };
        let measure = |text: &str| -> f32 {
            let run = TextRun {
                len: text.len(),
                font: font("Microsoft YaHei UI"),
                color: rgb(t.text).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(
                window
                    .text_system()
                    .shape_line(text.to_string().into(), px(13. * self.zoom), &[run], None)
                    .width,
            ) / self.zoom
                + 40.
        };
        let widths: Vec<_> = parts.iter().map(|(name, _)| measure(name)).collect();
        let hidden = rust_explorer::breadcrumb_layout::hidden_prefix(
            &widths,
            (available - 108.).max(0.),
            48.,
        );
        *self.hidden_breadcrumbs.borrow_mut() = parts.drain(..hidden).collect();
        let mut crumbs = div()
            .id("breadcrumbs")
            .flex_1()
            .min_w_0()
            .h(ui(32.))
            .bg(rgb(t.panel))
            .rounded(ui(4.))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .overflow_hidden()
            .cursor_pointer()
            .on_click(cx.listener(|v, _, w, cx| v.focus_address(w, cx)));
        crumbs = crumbs
            .child(
                div()
                    .w(ui(28.))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .child(visual::icon(
                        // The system glyph U+E7F4 in both cases: Segoe Fluent Icons
                        // on Windows 11, Segoe MDL2 Assets on Windows 10. The embedded
                        // SVG is only a fallback when neither font is installed.
                        "address-computer",
                        t.text,
                    )),
            )
            .child(visual::icon("chevron-right", t.muted));
        if hidden > 0 {
            crumbs = crumbs
                .child(
                    button("hidden-crumbs", "", t)
                        .w(ui(32.))
                        .px_1()
                        .child(visual::icon("more", t.text))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|v, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                v.popup = Some((
                                    "crumbs-menu".into(),
                                    point(e.position.x, e.position.y + px(24. * v.zoom)),
                                ));
                                v.submenu = None;
                                v.nested_submenu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(visual::icon("chevron-right", t.muted));
        }
        let n = parts.len();
        for (index, (label, path)) in parts.into_iter().enumerate() {
            crumbs = crumbs.child(
                div()
                    .id(("crumb", index))
                    .h_full()
                    .min_w_0()
                    .max_w(ui((available - 160.).max(24.)))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(ui(3.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(t.hover)))
                    .when(index + 1 < n, |d| d.flex_shrink_0())
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(label),
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        // Clicking an ancestor selects the folder we came from,
                        // like the ↑ button and Explorer's breadcrumbs.
                        let child = v.child_towards_current(&path);
                        v.navigate(path.clone(), cx);
                        if let Some(child) = child {
                            v.focus_after_load(child);
                        }
                    })),
            );
            crumbs = crumbs.child(visual::icon("chevron-right", t.muted));
        }
        crumbs
            .child(div().min_w(ui(64.)).h_full().flex_1())
            .into_any_element()
    }
    pub(super) fn save_settings(&self, _: &mut Context<Self>) {
        self.settings.enqueue_save();
    }
    pub(super) fn rebuild_rows(&mut self) {
        shell::menu_trace(format!(
            "view: rebuild_rows dir={:?} view={:?} group={:?} entries={} width={} zoom={}",
            self.dir(),
            self.view,
            self.group,
            self.entries.len(),
            self.layout_width,
            self.zoom
        ));
        self.thumbnail_viewport.set(None);
        self.rows.clear();
        self.content_image_present = false;
        if self.filter_folder != self.dir() {
            self.filter_folder = self.dir();
            self.filters = Default::default();
            self.collapsed_groups.clear();
        }
        let width = self.layout_width.max(200.);
        let cell = match self.view {
            ViewMode::ExtraLarge => 328.,
            ViewMode::Large => 164.,
            ViewMode::Medium => 124.,
            ViewMode::Small => 210.,
            ViewMode::List => 230.,
            ViewMode::Tiles => 290.,
            _ => width,
        };
        let columns = if matches!(self.view, ViewMode::Details | ViewMode::Content) {
            1
        } else {
            (width / cell).floor().max(1.) as usize
        };
        let mut groups = std::collections::BTreeMap::<String, Vec<usize>>::new();
        let today = chrono::Local::now().date_naive();
        let mut types = BTreeSet::new();
        for (index, e) in self.entries.iter().enumerate() {
            types.insert(e.kind.as_str());
            self.content_image_present |= is_image_entry(e);
            if !self.filters.matches(e, self.entry_size(e), today) {
                continue;
            }
            let key = match self.group.as_str() {
                "kind" => e.kind.clone(),
                "name" => e
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
                "modified" => e.date().split(' ').next().unwrap_or("未知日期").to_string(),
                "created" => format_date(e.created)
                    .split(' ')
                    .next()
                    .unwrap_or("未知日期")
                    .to_string(),
                "size" => {
                    let size = self.entry_size(e).unwrap_or(0);
                    if e.directory && !self.usage_enabled {
                        "文件夹"
                    } else if size < 1024 * 1024 {
                        "小于 1 MB"
                    } else if size < 100 * 1024 * 1024 {
                        "1–100 MB"
                    } else if size < 1024 * 1024 * 1024 {
                        "100 MB–1 GB"
                    } else {
                        "大于 1 GB"
                    }
                }
                .into(),
                _ => String::new(),
            };
            groups.entry(key).or_default().push(index);
        }
        let mut groups: Vec<_> = groups.into_iter().collect();
        self.filter_types = types.into_iter().map(str::to_owned).collect();
        self.group_members = groups.iter().cloned().collect();
        if !self.group.is_empty() {
            groups.sort_by(|a, b| fs::natural_cmp(&a.0, &b.0));
        }
        if self.view == ViewMode::List {
            let count = (self.layout_height / if self.settings.compact { 26. } else { 34. })
                .floor()
                .max(2.) as usize;
            self.rows = list_layout::rows(groups, count)
                .into_iter()
                .map(DisplayRow::List)
                .collect();
            self.retain_visible_selection();
            self.invalidate_list();
            return;
        }
        for (label, items) in groups {
            if !label.is_empty() {
                let collapsed = self.collapsed_groups.contains(&label);
                self.rows.push(DisplayRow::Group {
                    label,
                    count: items.len(),
                    collapsed,
                });
                if collapsed {
                    continue;
                }
            }
            for chunk in items.chunks(columns) {
                self.rows.push(DisplayRow::Items(chunk.to_vec()));
            }
        }
        self.retain_visible_selection();
        self.invalidate_list();
    }
    pub(super) fn visible_file_order(&self) -> Vec<usize> {
        if self.is_home() {
            return (0..self.entries.len()).collect();
        }
        if self.view == ViewMode::List {
            let columns = self
                .rows
                .iter()
                .filter_map(|row| {
                    if let DisplayRow::List(cells) = row {
                        Some(cells.len())
                    } else {
                        None
                    }
                })
                .max()
                .unwrap_or(0);
            let mut order = Vec::new();
            for column in 0..columns {
                for row in &self.rows {
                    if let DisplayRow::List(cells) = row {
                        if let Some(ListCell::File(index)) = cells.get(column) {
                            order.push(*index);
                        }
                    }
                }
            }
            order
        } else {
            self.rows
                .iter()
                .filter_map(|row| {
                    if let DisplayRow::Items(items) = row {
                        Some(items.iter().copied())
                    } else {
                        None
                    }
                })
                .flatten()
                .collect()
        }
    }
    fn retain_visible_selection(&mut self) {
        if self.is_home() {
            return;
        }
        let visible = self.visible_file_indices();
        let previous = self.selected.len();
        self.selected.retain(|index| visible.contains(index));
        if self.selected.len() != previous {
            self.clear_preview();
        }
        let visible_path = |path: &PathBuf| {
            self.entries
                .iter()
                .position(|e| &e.path == path)
                .is_some_and(|i| visible.contains(&i))
        };
        if self
            .keyboard_cursor
            .as_ref()
            .is_some_and(|path| !visible_path(path))
        {
            self.keyboard_cursor = None;
        }
        if self
            .focus_item
            .as_ref()
            .is_some_and(|path| !visible_path(path))
        {
            self.focus_item = None;
        }
        if !visible.contains(&self.anchor) {
            self.anchor = self.visible_file_order().first().copied().unwrap_or(0);
        }
    }
    pub(super) fn visible_file_indices(&self) -> BTreeSet<usize> {
        if self.is_home() {
            return (0..self.entries.len()).collect();
        }
        self.rows
            .iter()
            .flat_map(|row| match row {
                DisplayRow::Items(items) => items.clone(),
                DisplayRow::List(cells) => cells
                    .iter()
                    .filter_map(|cell| {
                        if let ListCell::File(index) = cell {
                            Some(*index)
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }
    fn group_is_selected(&self, label: &str) -> bool {
        self.group_members.get(label).is_some_and(|members| {
            !members.is_empty() && members.iter().all(|index| self.selected.contains(index))
        })
    }
    fn select_group(
        &mut self,
        label: &str,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.visible_file_indices();
        if let Some(members) = self.group_members.get(label) {
            self.selected = members
                .iter()
                .copied()
                .filter(|index| visible.contains(index))
                .collect();
            self.anchor = self.selected.first().copied().unwrap_or(0);
            self.focus_item = self
                .selected
                .first()
                .and_then(|i| self.entries.get(*i))
                .map(|e| e.path.clone());
            self.keyboard_cursor = None;
            self.rename_token += 1;
            self.load_preview(cx);
            window.focus(&self.focus);
        }
        if click_count == 2 && self.view != ViewMode::List {
            if !self.collapsed_groups.remove(label) {
                self.collapsed_groups.insert(label.to_owned());
            }
            self.rebuild_rows();
        }
        cx.notify();
    }
    fn entry_size(&self, e: &Entry) -> Option<u64> {
        if !e.directory {
            Some(e.size)
        } else {
            self.usage.get(&e.path).map(|u| u.bytes)
        }
    }
    pub(super) fn entry_size_text(&self, e: &Entry) -> String {
        if !e.directory {
            return e.size_label();
        }
        if !self.usage_enabled {
            return String::new();
        }
        match self.usage.get(&e.path) {
            Some(u) => format!(
                "{}{}",
                fs::size_label(u.bytes),
                if u.skipped > 0 || !u.complete {
                    " *"
                } else {
                    ""
                }
            ),
            None => if self.usage_running {
                "统计中…"
            } else {
                "未统计"
            }
            .into(),
        }
    }
    pub(super) fn start_usage(&mut self, cx: &mut Context<Self>) {
        if navigation::is_virtual(&self.dir()) || self.view != ViewMode::Details {
            return;
        }
        if self.usage_running {
            self.usage_cancel.store(true, Ordering::Relaxed);
            self.usage_revision = self.usage_revision.wrapping_add(1);
            self.usage_running = false;
            self.status = "统计已停止；* 表示跳过项目或部分结果".into();
            cx.notify();
            return;
        }
        self.usage_enabled = true;
        self.usage_running = true;
        self.usage.clear();
        self.usage_cancel = Arc::new(AtomicBool::new(false));
        self.usage_revision = self.usage_revision.wrapping_add(1);
        let cancel = self.usage_cancel.clone();
        let entries = self.entries.clone();
        let tx = self.usage_tx.clone();
        let generation = self.usage_revision;
        disk_usage::invalidate_paths(
            entries
                .iter()
                .filter(|e| e.directory)
                .map(|e| e.path.as_path()),
        );
        self.status = "后台统计当前层目录 · 不跨越联接/挂载点 · * 表示跳过或不完整".into();
        let task = cx.background_executor().spawn(async move {
            let mut skipped = 0;
            let mut hits = 0;
            for e in entries.iter().filter(|e| e.directory) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (usage, cached) = disk_usage::calculate_cached(&e.path, &cancel);
                hits += usize::from(cached);
                skipped += usage.skipped;
                let _ = tx.send((generation, e.path.clone(), usage));
            }
            (skipped, hits)
        });
        cx.spawn(async move |view, cx| {
            let (skipped, hits) = task.await;
            let _ = view.update(cx, |v, cx| {
                if v.usage_revision == generation {
                    v.usage_running = false;
                    v.status = format!(
                        "大小统计结束 · 缓存命中 {hits} 项（有效期 1 小时） · 跳过 {skipped} 项"
                    );
                    v.rebuild_rows();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn sort_current(&mut self, column: Sort, descending: bool) {
        self.sort = column;
        self.descending = descending;
        if volumes::is_computer(&self.dir()) || self.dir() == PathBuf::from("::Network") {
            self.settings.volume_sort = match column {
                Sort::Kind => "kind",
                Sort::Size => "total",
                _ => "name",
            }
            .into();
            self.settings.volume_descending = descending;
            self.sync_computer_entries();
            self.settings.enqueue_save();
            return;
        }
        let selected = self.paths();
        let usage = &self.usage;
        let folders_first = self.settings.folders_first;
        Arc::make_mut(&mut self.entries).sort_by(|a, b| {
            let dir = if column == Sort::Modified {
                a.directory.cmp(&b.directory)
            } else if folders_first {
                b.directory.cmp(&a.directory)
            } else {
                std::cmp::Ordering::Equal
            };
            dir.then_with(|| {
                if column == Sort::Size && self.usage_enabled {
                    let key = |e: &Entry| {
                        if e.directory {
                            usage.get(&e.path).map(|u| u.bytes)
                        } else {
                            Some(e.size)
                        }
                    };
                    let order = key(a)
                        .cmp(&key(b))
                        .then_with(|| fs::natural_cmp(&a.name, &b.name));
                    if descending { order.reverse() } else { order }
                } else {
                    compare_entries(a, b, column, descending)
                }
            })
        });
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| selected.contains(&e.path).then_some(i))
            .collect();
        self.rebuild_rows();
    }
    pub(super) fn detach_tab(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let tab = self.tabs[index].clone();
        if open_explorer(Some(tab), cx).is_ok() {
            self.remove_moved_tab(index, cx);
        }
    }
    pub(super) fn finish_tab_drag(&mut self, drop: TabDrop, cx: &mut Context<Self>) {
        use windows::Win32::{
            Foundation::POINT, Graphics::Gdi::ScreenToClient, UI::WindowsAndMessaging::*,
        };
        let Some(from) = self.tabs.iter().position(|tab| tab.id == drop.id) else {
            return;
        };
        let root = unsafe {
            GetAncestor(
                WindowFromPoint(POINT {
                    x: drop.position.0,
                    y: drop.position.1,
                }),
                GA_ROOT,
            )
        };
        let target = TAB_WINDOWS.with(|windows| windows.borrow().get(&(root.0 as isize)).cloned());
        if let Some(target) = target {
            let mut point = POINT {
                x: drop.position.0,
                y: drop.position.1,
            };
            let _ = unsafe { ScreenToClient(root, &mut point) };
            let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(root) } as f32 / 96.;
            let tab = self.tabs[from].clone();
            let self_id = cx.entity_id();
            if target.entity_id() == self_id {
                if point.y as f32 / (dpi.max(1.) * self.zoom) < 40. {
                    let index = self.tab_drop_index(point.x as f32 / (dpi.max(1.) * self.zoom));
                    let active_id = self.tabs[self.active].id;
                    let tab = self.tabs.remove(from);
                    let index = (if index > from { index - 1 } else { index }).min(self.tabs.len());
                    self.tabs.insert(index, tab);
                    self.active = self
                        .tabs
                        .iter()
                        .position(|t| t.id == active_id)
                        .unwrap_or(index);
                    cx.notify();
                    return;
                }
            } else {
                let moved = target
                    .update(cx, move |destination, cx| {
                        let y = point.y as f32 / (dpi.max(1.) * destination.zoom);
                        if !(0.0..40.0).contains(&y) {
                            return false;
                        }
                        let index = destination
                            .tab_drop_index(point.x as f32 / (dpi.max(1.) * destination.zoom));
                        destination.tabs.insert(index, tab);
                        destination.switch_tab(index, cx);
                        true
                    })
                    .unwrap_or(false);
                if moved {
                    self.remove_moved_tab(from, cx);
                    return;
                }
            }
        }
        self.detach_tab(drop.id, cx);
    }
    fn tab_drop_index(&self, x: f32) -> usize {
        let available = (self.window_width - 300.).max(110.);
        let width = (available / self.tabs.len().max(1) as f32 - 4.)
            .clamp(100., rust_explorer::layout_metrics::TAB_MAX_WIDTH);
        let arrows = if self.tabs.len() as f32 * (width + 4.) > available {
            24.
        } else {
            0.
        };
        let x = x - arrows - 8. - f32::from(self.tab_scroll.offset().x) / self.zoom;
        ((x + width / 2.) / (width + 4.))
            .floor()
            .max(0.)
            .min(self.tabs.len() as f32) as usize
    }
    fn remove_moved_tab(&mut self, index: usize, cx: &mut Context<Self>) {
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
    fn accept_tab(&mut self, drag: &DragTab, index: usize, cx: &mut Context<Self>) {
        drag.accepted.store(true, Ordering::Relaxed);
        if drag.source.entity_id() == cx.entity_id() {
            if let Some(from) = self.tabs.iter().position(|t| t.id == drag.tab.id) {
                let active_id = self.tabs[self.active].id;
                let tab = self.tabs.remove(from);
                let index = index.min(self.tabs.len());
                self.tabs.insert(index, tab);
                self.active = self
                    .tabs
                    .iter()
                    .position(|t| t.id == active_id)
                    .unwrap_or(index);
                cx.notify();
            }
        } else {
            let insert = index.min(self.tabs.len());
            self.tabs.insert(insert, drag.tab.clone());
            self.switch_tab(insert, cx);
            let id = drag.tab.id;
            let _ = drag.source.update(cx, |source, cx| {
                if let Some(from) = source.tabs.iter().position(|t| t.id == id) {
                    source.remove_moved_tab(from, cx);
                }
            });
        }
    }
    fn tab_full_path(&self, path: &std::path::Path) -> String {
        if navigation::is_shell_folder(path) {
            if self
                .shell_chain
                .last()
                .is_some_and(|(_, chain_path)| chain_path == path)
            {
                let mut out = navigation::computer_name();
                for (name, _) in &self.shell_chain {
                    out.push('\\');
                    out.push_str(name);
                }
                out
            } else {
                self.shell_names
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| navigation::location_name(path))
            }
        } else if navigation::is_app_virtual(path) {
            navigation::location_name(path)
        } else {
            path.display().to_string()
        }
    }
    fn tab_short_path(&self, path: &std::path::Path) -> String {
        let last = if navigation::is_shell_folder(path) {
            if self
                .shell_chain
                .last()
                .is_some_and(|(_, chain_path)| chain_path == path)
            {
                self.shell_chain.last().map(|(name, _)| name.clone())
            } else {
                self.shell_names.get(path).cloned()
            }
        } else {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        };
        last.map(|name| format!("..\\{name}"))
            .unwrap_or_else(|| self.tab_full_path(path))
    }
    fn show_tab_tip(
        &mut self,
        path: PathBuf,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !self.settings.full_path {
            self.hide_tab_tip();
            return;
        }
        let Some(hwnd) = native_hwnd(window) else {
            return;
        };
        use windows::Win32::{
            Foundation::{HWND, RECT},
            UI::WindowsAndMessaging::GetWindowRect,
        };
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(HWND(hwnd as _), &mut rect) }.is_err() {
            return;
        }
        let dpi = window.scale_factor();
        // Native text needs the user's zoom as well as the monitor DPI, the same rule
        // the file hover card uses.
        let scale = (f32::from(window.rem_size()) / 16.) * dpi;
        let text = self.tab_full_path(&path);
        let x = rect.left + (f32::from(bounds.left()) * dpi).round() as i32;
        let tab_bottom = rect.top + (f32::from(bounds.bottom()) * dpi).round() as i32;
        if self.tab_tip.is_none() || (self.tab_tip_scale - scale).abs() > 0.01 {
            self.tab_tip = crate::tab_tip::TabTip::new(Some(hwnd as isize), scale, self.theme.dark);
            self.tab_tip_scale = scale;
        }
        if let Some(tip) = &mut self.tab_tip {
            tip.show(&text, x, tab_bottom, scale);
        }
    }
    fn hide_tab_tip(&mut self) {
        if let Some(tip) = &self.tab_tip {
            tip.hide();
        }
    }
    pub(super) fn render_tabs(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let available = (f32::from(window.viewport_size().width) / self.zoom - 300.).max(110.);
        let tab_width = (available / self.tabs.len().max(1) as f32 - 4.)
            .clamp(100., rust_explorer::layout_metrics::TAB_MAX_WIDTH);
        let overflowing = self.tabs.len() as f32 * (tab_width + 4.) > available;
        let mut strip = div()
            .id("tab-strip")
            .w_full()
            .min_w_0()
            .relative()
            .pr(ui(138.))
            .h(ui(32.4))
            .flex_shrink_0()
            .flex()
            .items_end()
            .bg(rgb(t.chrome));
        let mut tabs = div()
            .id("tabs-scroll")
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll)
            .max_w(ui(available))
            .min_w_0()
            .flex()
            .items_end()
            .gap_1()
            .px_2();
        for (index, tab) in self.tabs.iter().enumerate() {
            let current = tab.history.current();
            let full_path = self.tab_full_path(current);
            let name = if self.settings.full_path {
                let short = self.tab_short_path(current);
                let text_space = (tab_width - 84.).max(16.) * self.zoom;
                let width = |text: &str| {
                    let run = TextRun {
                        len: text.len(),
                        font: font("Microsoft YaHei UI"),
                        color: rgb(t.text).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    f32::from(
                        window
                            .text_system()
                            .shape_line(text.to_string().into(), px(11.7 * self.zoom), &[run], None)
                            .width,
                    )
                };
                if width(&full_path) <= text_space {
                    full_path.clone()
                } else {
                    short
                }
            } else {
                self.shell_names
                    .get(current)
                    .cloned()
                    .unwrap_or_else(|| navigation::location_name(current))
            };
            let drag = DragTab {
                tab: tab.clone(),
                source: cx.entity().downgrade(),
                accepted: Arc::new(AtomicBool::new(false)),
            };
            let sender = self.drag_tx.clone();
            let ghost_name = name.clone();
            let tip_bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
            let tip_bounds_canvas = tip_bounds.clone();
            let tip_path = current.to_path_buf();
            tabs = tabs.child(
                div()
                    .id(("tab", tab.id))
                    .h(ui(32.4))
                    .w(ui(tab_width))
                    .flex_shrink_0()
                    .rounded_t(ui(7.))
                    .bg(rgb(if index == self.active {
                        t.active_tab()
                    } else {
                        t.chrome
                    }))
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_2()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |v, hovered, w, cx| {
                        if *hovered {
                            v.show_tab_tip(tip_path.clone(), tip_bounds.get(), w, cx);
                        } else {
                            v.hide_tab_tip();
                        }
                    }))
                    .on_click(cx.listener(move |v, _, _, cx| v.switch_tab(index, cx)))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |v, _, _, cx| v.close_tab(index, cx)),
                    )
                    .on_drag(drag, move |drag, _, _, cx| {
                        let flag = drag.accepted.clone();
                        let _ = drag.source.update(cx, |v, _| v.tab_drag = Some(flag));
                        cx.new(|_| TabGhost {
                            tab: drag.tab.clone(),
                            name: ghost_name.clone(),
                            accepted: drag.accepted.clone(),
                            width: tab_width,
                            theme: t,
                            sender: sender.clone(),
                        })
                    })
                    .on_drop(cx.listener(move |v, drag: &DragTab, _, cx| {
                        cx.stop_propagation();
                        v.accept_tab(drag, index, cx);
                    }))
                    .child(visual::icon_at(
                        if navigation::is_virtual(tab.history.current()) {
                            "computer"
                        } else {
                            "folder"
                        },
                        0xeac054,
                        if current == std::path::Path::new("::QuickAccess") {
                            12.
                        } else {
                            16.
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(ui(13. * 0.9))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .when(!self.settings.full_path, |d| d.text_ellipsis())
                            .child(name),
                    )
                    .child(
                        div()
                            .id(("close-tab", tab.id))
                            .child(visual::icon_at("close", t.text, 16. * 2. / 3. * 1.27))
                            .size(ui(24.))
                            .hover(move |s| s.bg(rgb(t.hover)))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(ui(3.))
                            .cursor_pointer()
                            .on_click(cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.close_tab(index, cx);
                            })),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| tip_bounds_canvas.set(bounds),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
        }
        if overflowing {
            strip = strip.child(button("tabs-left", "‹", t).w(ui(24.)).px_1().on_click(
                cx.listener(|v, _, _, cx| {
                    let offset = v.tab_scroll.offset();
                    v.tab_scroll.set_offset(point(
                        offset.x + px(rust_explorer::layout_metrics::TAB_MAX_WIDTH * v.zoom),
                        px(0.),
                    ));
                    cx.notify();
                }),
            ));
        }
        strip = strip.child(tabs);
        if overflowing {
            strip = strip.child(button("tabs-right", "›", t).w(ui(24.)).px_1().on_click(
                cx.listener(|v, _, _, cx| {
                    let offset = v.tab_scroll.offset();
                    v.tab_scroll.set_offset(point(
                        offset.x - px(rust_explorer::layout_metrics::TAB_MAX_WIDTH * v.zoom),
                        px(0.),
                    ));
                    cx.notify();
                }),
            ));
        }
        strip = strip
            .child(button("new-tab", "＋", t).on_click(cx.listener(|v, _, _, cx| v.new_tab(cx))))
            .child(
                div()
                    .id("caption-drag")
                    // Leave the top 4 px free of a drag hit region so the
                    // native top resize border still works; the caption
                    // buttons themselves remain flush with the window top.
                    .h(if window.is_maximized() {
                        ui(32.4)
                    } else {
                        ui(28.4)
                    })
                    .w(ui(64.))
                    .flex_1()
                    .min_w(ui(64.))
                    .window_control_area(WindowControlArea::Drag)
                    .on_drop(
                        cx.listener(|v, drag: &DragTab, _, cx| {
                            v.accept_tab(drag, v.tabs.len(), cx)
                        }),
                    ),
            );
        let mut controls = div()
            .id("window-controls")
            .absolute()
            .top_0()
            .right_0()
            .h_full()
            .w(ui(138.))
            .flex()
            .bg(rgb(t.chrome));
        for (id, _label, area) in [
            ("minimize", "─", WindowControlArea::Min),
            (
                "maximize",
                if window.is_maximized() { "❐" } else { "□" },
                WindowControlArea::Max,
            ),
            ("close-window", "×", WindowControlArea::Close),
        ] {
            controls = controls.child(
                div()
                    .id(id)
                    .h_full()
                    .w(ui(46.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(ui(16.))
                    .window_control_area(area)
                    .hover(move |s| {
                        s.bg(rgb(if id == "close-window" {
                            0xc42b1c
                        } else {
                            t.hover
                        }))
                    })
                    .child(visual::icon(
                        match id {
                            "minimize" => "window-minimize",
                            "maximize" => {
                                if window.is_maximized() {
                                    "window-restore"
                                } else {
                                    "window-maximize"
                                }
                            }
                            _ => "close",
                        },
                        t.text,
                    )),
            );
        }
        strip.child(controls).into_any_element()
    }
    pub(super) fn active_columns_mut(&mut self) -> &mut Vec<(String, f32)> {
        if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
            &mut self.settings.recycle_columns
        } else {
            &mut self.settings.columns
        }
    }
    pub(super) fn column_widths(&self, _available: f32) -> Vec<(String, f32)> {
        if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
            return self.settings.recycle_columns.clone();
        }
        self.settings.columns.clone()
    }
    pub(super) fn render_header(&self, available: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let widths = self.column_widths(available);
        let total: f32 = widths.iter().map(|(_, w)| *w).sum();
        let mut row = div()
            .h(ui(34.))
            .w(ui(total))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(t.border))
            .text_color(rgb(t.muted));
        for (index, (id, width)) in widths.into_iter().enumerate() {
            let column = column_sort(&id);
            let key = id.clone();
            let filter_key = id.clone();
            let filter_active = self.filters.active(&id);
            let filter_enabled = !self.is_home()
                && matches!(
                    id.as_str(),
                    "name" | "modified" | "created" | "kind" | "size"
                );
            let drag = DragColumn { id: id.clone() };
            row = row.child(
                div()
                    .id(("column", index))
                    .w(ui(width))
                    .h_full()
                    .flex_shrink_0()
                    .relative()
                    .flex()
                    .items_center()
                    .px_3()
                    .border_r_1()
                    .border_color(rgb(t.border))
                    .cursor_pointer()
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.sort_current(
                            column,
                            if v.sort == column {
                                !v.descending
                            } else {
                                false
                            },
                        );
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|v, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.popup = Some(("columns".into(), e.position));
                            cx.notify();
                        }),
                    )
                    .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_drop(cx.listener(move |v, drag: &DragColumn, _, cx| {
                        cx.stop_propagation();
                        if let Some(from) = v
                            .active_columns_mut()
                            .iter()
                            .position(|(id, _)| id == &drag.id)
                        {
                            let column = v.active_columns_mut().remove(from);
                            let columns = v.active_columns_mut();
                            columns.insert(index.min(columns.len()), column);
                            v.invalidate_list();
                            v.save_settings(cx);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(column_label(&key, self.usage_enabled)),
                    )
                    .when(self.sort == column, |d| {
                        d.child(visual::icon(
                            if self.descending {
                                "chevron-down"
                            } else {
                                "chevron-up"
                            },
                            t.muted,
                        ))
                    })
                    .when(filter_enabled, |d| {
                        d.child(
                            div()
                                .id(("column-filter", index))
                                .w(ui(20.))
                                .h_full()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(ui(3.))
                                .hover(move |s| s.bg(rgb(t.hover)))
                                .child(visual::icon(
                                    "chevron-down",
                                    if filter_active { t.accent } else { t.muted },
                                ))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        v.rename_token += 1;
                                        v.popup =
                                            Some((format!("filter-menu:{filter_key}"), e.position));
                                        v.submenu = None;
                                        v.nested_submenu = None;
                                        cx.notify();
                                    }),
                                )
                                .on_click(|_, _, cx| cx.stop_propagation()),
                        )
                    })
                    .child(
                        div()
                            .id(("resize-column", index))
                            .absolute()
                            .right_0()
                            .top_0()
                            .h_full()
                            .w(ui(8.))
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    v.columns_resize = Some((index, e.position.x.into(), width));
                                }),
                            )
                            .on_click(|_, _, cx| cx.stop_propagation()),
                    ),
            )
        }
        row.into_any_element()
    }
    fn displayed_name(&self, e: &Entry) -> String {
        if self.settings.extensions || e.directory {
            e.name.clone()
        } else {
            e.path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        }
    }
    fn name_element(
        &self,
        index: usize,
        e: &Entry,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.inline_rename.as_ref() == Some(&e.path) {
            let value = self.dialog_input.read(cx).value();
            let run = TextRun {
                len: value.len(),
                font: font("Microsoft YaHei UI"),
                color: rgb(self.theme.text).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let text_width = f32::from(
                window
                    .text_system()
                    .shape_line(
                        value.clone().into(),
                        px(13. * self.zoom),
                        &[run.clone()],
                        None,
                    )
                    .width,
            ) / self.zoom;
            let max_width =
                (f32::from(window.viewport_size().width) / self.zoom - 24.).clamp(48., 520.);
            let editor_width = (text_width + 20.).clamp(48., max_width);
            let lines = window
                .text_system()
                .shape_text(
                    value.into(),
                    px(13. * self.zoom),
                    &[run],
                    Some(px((editor_width - 8.) * self.zoom)),
                    None,
                )
                .ok()
                .and_then(|lines| lines.first().map(|line| line.wrap_boundaries().len() + 1))
                .unwrap_or(1);
            let editor_height = 4. + lines as f32 * 20.;
            // The editor is an overlay: growing it must not resize a details row.
            return div()
                .relative()
                .w(ui(width.max(24.)))
                .h(ui(22.))
                .flex_shrink_0()
                .child(
                    gpui::deferred(
                        gpui::anchored().snap_to_window().child(
                            div()
                                .id(("inline-rename", index))
                                .w(ui(editor_width))
                                .h(ui(editor_height))
                                .bg(rgb(self.theme.input))
                                .border_1()
                                .border_color(rgb(self.theme.accent))
                                .child(self.dialog_input.clone())
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                        ),
                    )
                    .with_priority(1),
                )
                .into_any_element();
        }
        let name = self.displayed_name(e);
        let source = cx.entity().downgrade();
        let drag = DragFiles {
            paths: if self.selected.contains(&index) {
                self.paths()
            } else {
                vec![e.path.clone()]
            },
        };
        let expand = matches!(
            self.view,
            ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium
        );
        div()
            .id(("filename", index))
            .w(ui(width.max(24.)))
            .flex_shrink_0()
            .min_w_0()
            .line_height(ui(20.))
            .when(expand, |d| d.whitespace_normal())
            .when(!expand, |d| {
                d.whitespace_nowrap().overflow_hidden().text_ellipsis()
            })
            .child(if expand {
                icon_label(&name, width, 13., self.zoom, window)
            } else {
                div().child(name).into_any_element()
            })
            .on_drag(drag, move |drag, _, w, cx| {
                let _ = source.update(cx, |v, cx| {
                    v.rename_token += 1;
                    v.pending_selection_click = None;
                    v.update_file_drag(drag.paths.clone(), w.mouse_position(), w.modifiers(), cx);
                });
                cx.new(|_| drag.clone())
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    if v.activate_file_item(index, e, cx) {
                        return;
                    }
                    let rename = v.selected.len() == 1
                        && v.selected.contains(&index)
                        && v.selected_at.elapsed() > Duration::from_millis(500)
                        && !e.modifiers.control
                        && !e.modifiers.shift
                        && e.click_count == 1
                        && !v.settings.single_click;
                    if v.selected.contains(&index)
                        && !e.modifiers.shift
                        && e.click_count == 1
                        && !v.settings.single_click
                    {
                        v.pending_selection_click = Some(index);
                    } else {
                        v.select(index, e.modifiers.control, e.modifiers.shift, w, cx);
                    }
                    if e.click_count == 2 || v.settings.single_click {
                        v.rename_token += 1;
                        v.open_selected_with(e.modifiers, cx);
                    } else if rename {
                        let token = v.rename_token;
                        cx.spawn(async move |view, cx| {
                            cx.background_executor()
                                .timer(Duration::from_millis(550))
                                .await;
                            let _ = view.update(cx, |v, cx| {
                                if v.rename_token == token
                                    && v.selected.len() == 1
                                    && v.selected.contains(&index)
                                {
                                    v.pending_rename = true;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                }),
            )
            .into_any_element()
    }
    fn file_item(
        &self,
        index: usize,
        cell_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.entries.get(index) else {
            return div().into_any_element();
        };
        let t = self.theme;
        let hint_entry = entry.clone();
        let details = self.view == ViewMode::Details;
        let large = matches!(
            self.view,
            ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium
        );
        let rich = self.view == ViewMode::Tiles;
        // Keep grid allocation stable, but paint each item's own content rectangle.
        let natural_name = self.displayed_name(entry);
        let run = TextRun {
            len: natural_name.len(),
            font: font("Microsoft YaHei UI"),
            color: rgb(t.text).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let text_width = f32::from(
            window
                .text_system()
                .shape_line(
                    natural_name.clone().into(),
                    px(13. * self.zoom),
                    &[run],
                    None,
                )
                .width,
        ) / self.zoom;
        let item_width = if large {
            (text_width + 24.)
                .max(self.view.icon_size() + 24.)
                .min(cell_width)
        } else {
            cell_width
        };
        let name_lines = if large {
            icon_label_lines(
                &natural_name,
                (item_width - 24.).max(24.),
                13.,
                self.zoom,
                window,
            )
            .len() as f32
        } else {
            1.
        };
        let drag_source = cx.entity().downgrade();
        let self_selected = self.selected.contains(&index);
        let dragging_files = self.file_drag.is_some();
        let drop_target =
            self.file_drag.as_ref().and_then(|d| d.target.as_ref()) == Some(&entry.path);
        let drop_bounds = self.file_drop_bounds.clone();
        let drop_path = entry.path.clone();
        let drop_directory = entry.directory;
        let painted_bounds = std::rc::Rc::new(std::cell::Cell::new(Bounds::<Pixels>::default()));
        let measure = |text: String| {
            let run = TextRun {
                len: text.len(),
                font: font("Microsoft YaHei UI"),
                color: rgb(t.text).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            f32::from(
                window
                    .text_system()
                    .shape_line(text.into(), px(13. * self.zoom), &[run], None)
                    .width,
            ) / self.zoom
        };
        let mut hit_regions: Vec<(f32, f32, f32, f32)> = Vec::new();
        if details {
            let height = if self.settings.compact {
                rust_explorer::layout_metrics::COMPACT_FILE_ROW
            } else {
                34.
            };
            let mut x = 0.;
            for (id, width) in self.column_widths(self.layout_width) {
                let (text, extra) = match id.as_str() {
                    "name" => (
                        natural_name.clone(),
                        28. + if self.settings.checkboxes { 30. } else { 0. },
                    ),
                    "modified" => (entry.date(), 0.),
                    "created" => (format_date(entry.created), 0.),
                    "kind" => (entry.kind.clone(), 0.),
                    "size" => (self.entry_size_text(entry), 0.),
                    "original" => (
                        entry
                            .recycle
                            .as_ref()
                            .map(|i| i.original_location.clone())
                            .unwrap_or_default(),
                        0.,
                    ),
                    "deleted" => (
                        format_date(entry.recycle.as_ref().and_then(|i| i.deleted)),
                        0.,
                    ),
                    _ => (String::new(), 0.),
                };
                if !text.is_empty() || extra > 0. {
                    let occupied = (measure(text) + extra + 8.).min((width - 16.).max(0.));
                    let left = if id == "size" {
                        x + width - 12. - occupied
                    } else {
                        x + 8.
                    };
                    hit_regions.push((
                        left,
                        ((height - 26.) / 2.).max(0.),
                        occupied,
                        height.min(26.),
                    ));
                }
                x += width;
            }
        } else if large {
            let icon = self.view.icon_size();
            hit_regions.push(((item_width - icon) / 2. - 4., 0., icon + 8., icon + 8.));
            let width = (text_width + 12.).min(item_width - 16.);
            hit_regions.push((
                (item_width - width) / 2.,
                icon + 8.,
                width,
                name_lines * 20. + 8.,
            ));
        } else if self.view == ViewMode::Content {
            let presentation = rust_explorer::thumbnails::Key::from_entry(
                entry,
                visual::thumbnail_pixels(self.view.icon_size()),
            )
            .and_then(|key| self.thumbnails.presentation(&key))
            .unwrap_or_default();
            let image = presentation.source_width > 0 && presentation.source_height > 0;
            let inner = (item_width - 24.).max(0.);
            let (first, visible) = rust_explorer::layout_metrics::content_columns(inner, image);
            let icon = self.view.icon_size();
            hit_regions.push((8., (74. - icon) / 2. - 4., icon + 8., icon + 8.));
            let title_width = text_width.max(if entry.directory || image {
                0.
            } else {
                measure(entry.kind.clone())
            });
            hit_regions.push((
                12. + icon + 8.,
                12.,
                (title_width + 8.).min((first - icon - 8.).max(0.)),
                50.,
            ));
            if visible {
                let metadata = (inner - first - 16.).clamp(1., 240.);
                let text = if image {
                    measure(format!("大小：{}", self.entry_size_text(entry)))
                } else {
                    measure(format!("修改日期：{}", entry.date()))
                        .max(measure(format!("大小：{}", self.entry_size_text(entry))))
                };
                hit_regions.push((
                    item_width - 12. - metadata,
                    10.,
                    (text + 8.).min(metadata),
                    54.,
                ));
                if image {
                    let middle = (inner - first - metadata - 16.).max(1.);
                    let text = measure(format!("类型：{}", entry.kind)).max(measure(format!(
                        "分辨率：{} x {}",
                        presentation.source_width, presentation.source_height
                    )));
                    hit_regions.push((first + 20., 10., (text + 8.).min(middle), 54.));
                }
            }
        } else if self.view == ViewMode::Tiles {
            hit_regions.push((
                8.,
                4.,
                (self.view.icon_size()
                    + text_width.max(measure(format!(
                        "{}  {}",
                        entry.kind,
                        self.entry_size_text(entry)
                    )))
                    + 24.)
                    .min(item_width - 16.),
                66.,
            ));
        } else {
            hit_regions.push((
                8.,
                0.,
                (text_width + 40.).min(item_width - 16.),
                if self.settings.compact { 26. } else { 34. },
            ));
        }
        let right_regions = std::rc::Rc::new(hit_regions);
        let down_regions = right_regions.clone();
        let down_bounds = painted_bounds.clone();
        let up_bounds = painted_bounds.clone();
        let zoom = self.zoom;
        // A selected item answers a right click anywhere inside its own cell;
        // an unselected one still requires a text/icon/metadata hit, so
        // right-clicking the empty part of another row keeps the background
        // menu and does not steal the selection.
        let hit_test = move |v: &Self,
                             bounds: Bounds<Pixels>,
                             position: Point<Pixels>,
                             regions: &[(f32, f32, f32, f32)]| {
            let local = position - bounds.origin;
            let (x, y) = (f32::from(local.x) / zoom, f32::from(local.y) / zoom);
            let (width, height) = (
                f32::from(bounds.size.width) / zoom,
                f32::from(bounds.size.height) / zoom,
            );
            rust_explorer::layout_metrics::right_click_hits(
                regions,
                x,
                y,
                width,
                height,
                v.selected.contains(&index),
            )
        };
        let mut row = div()
            .id(("file", index))
            .when(!dragging_files, |d| {
                d.tooltip(move |_, cx| cx.new(|cx| NameTip::new(hint_entry.clone(), t, cx)).into())
            })
            .w(ui(item_width))
            .min_w_0()
            .flex_shrink_0()
            .relative()
            .min_h(ui(if large {
                self.view.icon_size() + 16. + name_lines * 20.
            } else if rich {
                74.
            } else if self.settings.compact {
                rust_explorer::layout_metrics::COMPACT_FILE_ROW
            } else {
                34.
            }))
            .py_1()
            .when(details, |d| {
                d.h(ui(if self.settings.compact {
                    rust_explorer::layout_metrics::COMPACT_FILE_ROW
                } else {
                    34.
                }))
                .py_0()
            })
            .when(self.view == ViewMode::Content, |d| d.h(ui(74.)).py_0())
            .flex()
            .items_center()
            .when(large, |d| {
                d.flex_shrink_0()
                    .h(ui(self.view.icon_size() + 16. + name_lines * 20.))
            })
            .rounded(ui(3.))
            .bg(rgb(if self.selected.contains(&index) || drop_target {
                t.selection
            } else {
                t.background
            }))
            .when(!dragging_files || entry.directory, |d| {
                d.hover(move |s| {
                    s.bg(rgb(if self_selected || drop_target {
                        if t.dark { 0x465569 } else { 0xb8d7f3 }
                    } else {
                        t.hover
                    }))
                })
            })
            .when(entry.hidden, |d| d.opacity(0.5))
            .cursor_pointer()
            .when(self.inline_rename.is_none(), |d| {
                d.on_drag(
                    DragFiles {
                        paths: if self.selected.contains(&index) {
                            self.paths()
                        } else {
                            vec![entry.path.clone()]
                        },
                    },
                    move |drag, _, w, cx| {
                        let _ = drag_source.update(cx, |v, cx| {
                            v.rename_token += 1;
                            v.pending_selection_click = None;
                            v.update_file_drag(
                                drag.paths.clone(),
                                w.mouse_position(),
                                w.modifiers(),
                                cx,
                            );
                        });
                        cx.new(|_| drag.clone())
                    },
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    if v.activate_file_item(index, e, cx) {
                        return;
                    }
                    if v.selected.contains(&index)
                        && !e.modifiers.shift
                        && e.click_count == 1
                        && !v.settings.single_click
                    {
                        v.pending_selection_click = Some(index);
                    } else {
                        v.select(index, e.modifiers.control, e.modifiers.shift, w, cx);
                    }
                    if e.click_count == 2 || v.settings.single_click {
                        v.open_selected_with(e.modifiers, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                    if !hit_test(v, down_bounds.get(), e.position, &down_regions) {
                        return;
                    }
                    cx.stop_propagation();
                    v.rename_token += 1;
                    let paths = if v.selected.contains(&index) {
                        v.paths()
                    } else {
                        vec![v.entries[index].path.clone()]
                    };
                    v.right_file_drag = Some(RightFileDrag {
                        index,
                        paths,
                        start: e.position,
                        active: false,
                    });
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |v, e: &MouseUpEvent, w, cx| {
                    if !hit_test(v, up_bounds.get(), e.position, &right_regions) {
                        return;
                    }
                    cx.stop_propagation();
                    v.rename_token += 1;
                    if !v.selected.contains(&index) {
                        v.select(index, false, false, w, cx);
                    }
                    if e.modifiers.shift {
                        v.system_menu(cx);
                        return;
                    }
                    v.menu(cx);
                    if let Some((_, position)) = &mut v.popup {
                        *position = e.position;
                    }
                }),
            );
        if entry.directory {
            let path = entry.path.clone();
            let external = path.clone();
            row = row
                .on_drop(cx.listener(move |v, drag: &DragFiles, w, cx| {
                    cx.stop_propagation();
                    v.confirm_drop(drag.paths.clone(), path.clone(), w.modifiers().control, cx);
                }))
                .on_drop(cx.listener(move |v, drag: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    v.confirm_drop(drag.paths().to_vec(), external.clone(), false, cx);
                }));
        } else {
            // A file is not a destination; consume the drop so it cannot
            // accidentally bubble to the surrounding directory background.
            row = row
                .on_drop(|_: &DragFiles, _, cx| cx.stop_propagation())
                .on_drop(|_: &ExternalPaths, _, cx| cx.stop_propagation());
        }
        let checkbox = |this: &Self, cx: &mut Context<Self>| {
            div()
                .id(("check", index))
                .w(ui(22.))
                .flex_shrink_0()
                .child(visual::checkbox(this.selected.contains(&index), t.muted))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, _, w, cx| {
                        cx.stop_propagation();
                        v.select(index, true, false, w, cx);
                    }),
                )
        };
        if details {
            for (id, width) in self.column_widths(self.layout_width) {
                let cell = div().w(ui(width)).flex_shrink_0().px_3().when(
                    id != "name" || self.inline_rename.as_ref() != Some(&entry.path),
                    |d| d.overflow_hidden(),
                );
                row = row.child(if id == "name" {
                    cell.flex()
                        .items_center()
                        .gap_2()
                        .when(self.settings.checkboxes, |d| d.child(checkbox(self, cx)))
                        .child(visual::entry_icon(
                            &entry.path,
                            entry.directory,
                            entry.link,
                            t.muted,
                            16.,
                        ))
                        .child(self.name_element(
                            index,
                            entry,
                            width - 48. - if self.settings.checkboxes { 30. } else { 0. },
                            window,
                            cx,
                        ))
                        .into_any_element()
                } else {
                    let value = match id.as_str() {
                        "modified" => entry.date(),
                        "created" => format_date(entry.created),
                        "kind" => entry.kind.clone(),
                        "size" if entry.recycle.is_some() => fs::size_label(entry.size),
                        "original" => entry
                            .recycle
                            .as_ref()
                            .map(|info| info.original_location.clone())
                            .unwrap_or_default(),
                        "deleted" => {
                            format_date(entry.recycle.as_ref().and_then(|info| info.deleted))
                        }
                        "size" => self.entry_size_text(entry),
                        _ => String::new(),
                    };
                    cell.whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(t.muted))
                        .when(id == "size", |d| d.text_right())
                        .child(value)
                        .into_any_element()
                });
            }
        } else {
            row = row
                .px_3()
                .gap_2()
                .when(large, |d| d.flex_col().justify_start());
            if self.settings.checkboxes {
                row = if large {
                    row.child(
                        div()
                            .absolute()
                            .left(ui(3.))
                            .top(ui(3.))
                            .child(checkbox(self, cx)),
                    )
                } else {
                    row.child(checkbox(self, cx))
                };
            }
            let key = if large || self.view == ViewMode::Content {
                rust_explorer::thumbnails::Key::from_entry(
                    entry,
                    visual::thumbnail_pixels(self.view.icon_size()),
                )
            } else {
                None
            };
            let thumbnail = key.as_ref().and_then(|key| self.thumbnails.image(key));
            let presentation = key
                .as_ref()
                .and_then(|key| self.thumbnails.presentation(key))
                .unwrap_or_default();
            let has_thumbnail = thumbnail.is_some();
            let picture = div()
                .relative()
                .size(ui(self.view.icon_size()))
                .flex_shrink_0()
                .child(visual::thumbnail_or_icon(
                    &entry.path,
                    entry.directory,
                    t.muted,
                    self.view.icon_size(),
                    thumbnail,
                    (presentation.width, presentation.height),
                    entry.directory || presentation.directory,
                    &self.thumbnail_images,
                    cx,
                ))
                .when(has_thumbnail && presentation.video, |d| {
                    d.child(visual::film_strip()).child(
                        div()
                            .absolute()
                            .right_0()
                            .bottom_0()
                            .size(ui(20.))
                            .child(visual::associated_app_icon(&entry.path, t.text, 20.)),
                    )
                })
                .when(
                    (has_thumbnail && presentation.shortcut) || entry.link,
                    |d| d.child(visual::link_badge(self.view.icon_size())),
                );
            if self.view == ViewMode::Content {
                let inner = (item_width - 24.).max(0.);
                // 160 logical pixels = 280 physical pixels at 175% DPI.
                let image = presentation.source_width > 0 && presentation.source_height > 0;
                let (first, show_metadata) =
                    rust_explorer::layout_metrics::content_columns(inner, image);
                let text_width = (first - self.view.icon_size() - 8.).max(1.);
                let metadata_width = (inner - first - 16.).clamp(1., 240.);
                let image_width = (inner - first - metadata_width - 16.).max(1.);
                let ellipsis = |text: String, width: f32| {
                    div()
                        .w(ui(width))
                        .flex_shrink_0()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(text)
                };
                let mut title = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(self.name_element(index, entry, text_width, window, cx));
                if !entry.directory && !image {
                    title = title.child(
                        ellipsis(entry.kind.clone(), text_width)
                            .text_color(rgb(t.muted))
                            .text_size(ui(12.)),
                    );
                }
                row = row.child(
                    div()
                        .w(ui(first))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .overflow_hidden()
                        .child(picture)
                        .child(title),
                );
                if show_metadata {
                    if image {
                        row = row.child(
                            div()
                                .w(ui(image_width))
                                .flex_shrink_0()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .text_size(ui(12.))
                                .text_color(rgb(t.muted))
                                .child(ellipsis(format!("类型：{}", entry.kind), image_width))
                                .child(ellipsis(
                                    format!(
                                        "分辨率：{} x {}",
                                        presentation.source_width, presentation.source_height
                                    ),
                                    image_width,
                                )),
                        );
                    } else {
                        row = row.child(div().flex_1());
                    }
                    let mut metadata = div()
                        .w(ui(metadata_width))
                        .min_w_0()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .text_size(ui(12.))
                        .text_color(rgb(t.muted));
                    if !image {
                        metadata = metadata.child(ellipsis(
                            format!("修改日期：{}", entry.date()),
                            metadata_width,
                        ));
                    }
                    if !entry.directory {
                        metadata = metadata.child(ellipsis(
                            format!("大小：{}", self.entry_size_text(entry)),
                            metadata_width,
                        ));
                    }
                    row = row.child(metadata);
                }
            } else {
                row = row.child(picture);
                let name_width = (item_width
                    - 24.
                    - if large {
                        0.
                    } else {
                        self.view.icon_size() + 8.
                    }
                    - if self.settings.checkboxes && !large {
                        30.
                    } else {
                        0.
                    })
                .max(24.);
                let mut name = div()
                    .w(ui(name_width))
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when(large, |d| d.w_full().text_center())
                    .child(self.name_element(index, entry, name_width, window, cx));
                if rich {
                    name = name.child(
                        div()
                            .text_color(rgb(t.muted))
                            .text_size(ui(12.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(format!("{}  {}", entry.kind, self.entry_size_text(entry))),
                    );
                    if self.view == ViewMode::Content {
                        name = name.child(
                            div()
                                .text_color(rgb(t.muted))
                                .text_size(ui(12.))
                                .child(format!("修改日期：{}", entry.date())),
                        );
                    }
                }
                row = row.child(name);
            }
        }
        let row = row
            .when(
                self.focus_item.as_ref() == Some(&entry.path)
                    && (!self.selected.contains(&index) || self.selected.len() > 1),
                |d| d.child(visual::focus_outline(t.muted)),
            )
            .child(
                canvas(
                    move |bounds, _, _| {
                        painted_bounds.set(bounds);
                        drop_bounds
                            .borrow_mut()
                            .push((drop_path.clone(), bounds, drop_directory));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        if large {
            div()
                .w(ui(cell_width))
                .flex_shrink_0()
                .flex()
                .items_start()
                .justify_center()
                .child(row)
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }
    pub(super) fn render_list_row(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_theme = self.theme;
        // GPUI holds ListState's mutable borrow while asking us to draw rows.
        // Read the final viewport once after layout, never from this callback.
        if !self.thumbnail_sync_queued.replace(true) {
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |view, _| {
                    view.thumbnail_sync_queued.set(false);
                    view.sync_thumbnails();
                });
            });
        }
        match self.rows.get(index) {
            Some(DisplayRow::List(cells)) => {
                let first = ((-f32::from(self.content_scroll.offset().x) / self.zoom) / 230.)
                    .floor()
                    .max(0.) as usize;
                let count = (self.layout_width / 230.).ceil() as usize + 2;
                let height = if self.settings.compact { 26. } else { 34. };
                div()
                    .flex()
                    .w(ui(230. * cells.len() as f32))
                    .h(ui(height))
                    .child(div().w(ui(first as f32 * 230.)).flex_shrink_0())
                    .children(cells.iter().skip(first).take(count).enumerate().map(
                        |(column, cell)| {
                            match cell {
                                ListCell::File(i) => self.file_item(*i, 230., window, cx),
                                ListCell::Group(name) => {
                                    let label = name.clone();
                                    let selected = self.group_is_selected(name);
                                    let count =
                                        self.group_members.get(name).map(Vec::len).unwrap_or(0);
                                    div()
                                        .id(SharedString::from(format!(
                                            "list-group-{index}-{}",
                                            first + column
                                        )))
                                        .w(ui(230.))
                                        .h(ui(height))
                                        .px_3()
                                        .flex_shrink_0()
                                        .whitespace_nowrap()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .text_color(rgb(self.theme.text))
                                        .bg(rgb(if selected {
                                            group_theme.selection
                                        } else {
                                            group_theme.background
                                        }))
                                        .hover(move |s| {
                                            s.bg(rgb(if selected {
                                                if group_theme.dark { 0x465569 } else { 0xb8d7f3 }
                                            } else {
                                                group_theme.hover
                                            }))
                                        })
                                        .child(visual::icon("chevron-down", self.theme.text))
                                        .child(format!("{name} ({count})"))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                                                cx.stop_propagation();
                                                v.select_group(&label, e.click_count, w, cx);
                                            }),
                                        )
                                        .into_any_element()
                                }
                                ListCell::Empty => div()
                                    .w(ui(230.))
                                    .h(ui(height))
                                    .flex_shrink_0()
                                    .into_any_element(),
                            }
                        },
                    ))
                    .into_any_element()
            }
            Some(DisplayRow::Group {
                label,
                count,
                collapsed,
            }) => {
                let key = label.clone();
                let selected = self.group_is_selected(label);
                div()
                    .id(SharedString::from(format!("group:{label}")))
                    .h(ui(38.))
                    .w_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(rgb(self.theme.text))
                    .bg(rgb(if selected {
                        group_theme.selection
                    } else {
                        group_theme.background
                    }))
                    .hover(move |s| {
                        s.bg(rgb(if selected {
                            if group_theme.dark { 0x465569 } else { 0xb8d7f3 }
                        } else {
                            group_theme.hover
                        }))
                    })
                    .child(visual::icon(
                        if *collapsed {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        self.theme.text,
                    ))
                    .child(format!("{label} ({count})"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                            cx.stop_propagation();
                            v.select_group(&key, e.click_count, w, cx);
                        }),
                    )
                    .into_any_element()
            }
            Some(DisplayRow::Items(items)) => {
                let columns = match self.view {
                    ViewMode::Details | ViewMode::Content => 1,
                    _ => {
                        let cell = match self.view {
                            ViewMode::ExtraLarge => 328.,
                            ViewMode::Large => 164.,
                            ViewMode::Medium => 124.,
                            ViewMode::Small => 210.,
                            ViewMode::List => 230.,
                            ViewMode::Tiles => 290.,
                            _ => self.layout_width,
                        };
                        (self.layout_width / cell).floor().max(1.) as usize
                    }
                };
                let width = if self.view == ViewMode::List {
                    230.
                } else if self.view == ViewMode::Details {
                    self.column_widths(self.layout_width)
                        .iter()
                        .map(|(_, w)| *w)
                        .sum()
                } else {
                    self.layout_width / columns as f32
                };
                if self.view == ViewMode::List {
                    let first = ((-f32::from(self.content_scroll.offset().x) / self.zoom) / width)
                        .floor()
                        .max(0.) as usize;
                    let visible = (self.layout_width / width).ceil() as usize + 2;
                    return div()
                        .flex()
                        .w(ui(width * items.len() as f32))
                        .child(div().w(ui(first as f32 * width)).flex_shrink_0())
                        .children(
                            items
                                .iter()
                                .skip(first)
                                .take(visible)
                                .map(|i| self.file_item(*i, width, window, cx)),
                        )
                        .into_any_element();
                }
                div()
                    .flex()
                    .children(items.iter().map(|i| self.file_item(*i, width, window, cx)))
                    .into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}
pub(super) fn format_date(time: Option<std::time::SystemTime>) -> String {
    time.map(|t| {
        chrono::DateTime::<chrono::Local>::from(t)
            .format("%Y/%m/%d %H:%M")
            .to_string()
    })
    .unwrap_or_default()
}

struct MenuItem {
    id: String,
    label: String,
    checked: bool,
    enabled: bool,
}
impl MenuItem {
    fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked: false,
            enabled: true,
        }
    }
    fn check(mut self, value: bool) -> Self {
        self.checked = value;
        self
    }
    fn enable(mut self, value: bool) -> Self {
        self.enabled = value;
        self
    }
    fn separator() -> Self {
        Self::new("-", "")
    }
}
impl Explorer {
    fn menu_items(&self, kind: &str, cx: &App) -> Vec<MenuItem> {
        if let Some(column) = kind.strip_prefix("filter-menu:") {
            let choices: Vec<(String, String)> = match column {
                "name" => rust_explorer::file_filter::NAMES
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
                "modified" | "created" => rust_explorer::file_filter::DATES
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
                "size" => rust_explorer::file_filter::SIZES
                    .iter()
                    .map(|(k, v)| ((*k).into(), (*v).into()))
                    .collect(),
                "kind" => self
                    .filter_types
                    .iter()
                    .map(|s| (s.clone(), s.clone()))
                    .collect(),
                _ => Vec::new(),
            };
            return std::iter::once(
                MenuItem::new(format!("filter-clear:{column}"), "清除此列筛选")
                    .enable(self.filters.active(column)),
            )
            .chain(std::iter::once(MenuItem::separator()))
            .chain(choices.into_iter().map(|(key, label)| {
                MenuItem::new(format!("filter-option:{column}:{key}"), label)
                    .check(self.filters.checked(column, &key))
            }))
            .chain([
                MenuItem::separator(),
                MenuItem::new("filter-reset", "清除所有筛选"),
                MenuItem::new("filter-done", "完成"),
            ])
            .collect();
        }
        match kind {
            "system-operations" => self
                .system_operations
                .iter()
                .rev()
                .flat_map(|op| {
                    let elapsed = op
                        .finished
                        .unwrap_or_else(Instant::now)
                        .duration_since(op.started)
                        .as_secs_f32();
                    let mut items = vec![
                        MenuItem::new(
                            "operation-info",
                            format!("{} · {:.1}秒 · {}", op.label, elapsed, op.status),
                        )
                        .enable(false),
                    ];
                    if op.finished.is_none() {
                        items.push(MenuItem::new(
                            format!("operation-stop:{}", op.id),
                            "停止（请求关闭系统窗口）",
                        ));
                        items.push(MenuItem::new(
                            format!("operation-force:{}", op.id),
                            "强行停止（可能已完成部分操作）",
                        ));
                    }
                    items.push(MenuItem::separator());
                    items
                })
                .collect(),
            "text-menu" => [
                ("cut", "剪切    Ctrl+X"),
                ("copy", "复制    Ctrl+C"),
                ("paste", "粘贴    Ctrl+V"),
                ("all", "全选    Ctrl+A"),
            ]
            .into_iter()
            .filter(|(id, _)| {
                let can_paste = *id == "paste"
                    && cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .is_some_and(|text| !text.is_empty());
                self.text_menu_target
                    .as_ref()
                    .is_some_and(|input| input.read(cx).text_command_available(id, can_paste))
            })
            .map(|(id, label)| MenuItem::new(format!("text:{id}"), label))
            .collect(),
            "drop-menu" => vec![
                MenuItem::new("drop:copy", "复制到这里"),
                MenuItem::new("drop:move", "移动到这里"),
                MenuItem::new("drop:link", "在此创建快捷方式"),
                MenuItem::separator(),
                MenuItem::new("drop:cancel", "取消"),
            ],
            "crumbs-menu" => self
                .hidden_breadcrumbs
                .borrow()
                .iter()
                .enumerate()
                .rev()
                .map(|(i, (label, _))| MenuItem::new(format!("hidden-crumb:{i}"), label.clone()))
                .collect(),
            "file-menu" => {
                let folder = self.selected.len() == 1
                    && self
                        .selected
                        .first()
                        .is_some_and(|i| self.entries[*i].directory);
                if navigation::is_shell_folder(&self.dir()) {
                    let single = self.selected.len() == 1;
                    let mut items = vec![MenuItem::new("file-open", "打开")];
                    if folder {
                        items.push(MenuItem::new("file-tab", "在新标签页中打开"));
                        items.push(MenuItem::new("file-window", "在新窗口中打开"));
                    }
                    items.push(MenuItem::separator());
                    if folder {
                        items.push(MenuItem::new("file-cut", "剪切"));
                        items.push(MenuItem::new("file-copy", "复制"));
                    } else {
                        items.push(MenuItem::new("file-copy", "复制"));
                        items.push(MenuItem::new("file-cut", "剪切"));
                    }
                    items.push(MenuItem::new("file-delete", "删除"));
                    items.push(MenuItem::new("file-rename", "重命名").enable(single));
                    items.push(MenuItem::separator());
                    items.push(MenuItem::new("properties", "属性").enable(single));
                    return items;
                }
                vec![
                    MenuItem::new("file-open", "打开"),
                    MenuItem::new("file-tab", "在新标签页中打开").enable(folder),
                    MenuItem::new("file-window", "在新窗口中打开").enable(folder),
                    MenuItem::new("file-explorer", "在系统资源管理器中打开").enable(folder),
                    MenuItem::separator(),
                    MenuItem::new("file-cut", "剪切"),
                    MenuItem::new("file-copy", "复制"),
                    MenuItem::new("file-rename", "重命名").enable(
                        self.selected.len() == 1
                            && (!navigation::is_virtual(&self.dir())
                                || self
                                    .selected
                                    .first()
                                    .and_then(|index| self.entries.get(*index))
                                    .is_some_and(|entry| {
                                        rust_explorer::volume_label::is_drive_root(&entry.path)
                                    })),
                    ),
                    MenuItem::new("file-delete", "删除")
                        .enable(!navigation::is_app_virtual(&self.dir())),
                    MenuItem::new("copy-path", "复制路径"),
                    MenuItem::new("pin", "固定到快速访问")
                        .enable(folder && !navigation::is_virtual(&self.dir())),
                    MenuItem::new("submenu:access-menu", "授予访问权限                 ›")
                        .enable(folder && !navigation::is_virtual(&self.dir())),
                    MenuItem::separator(),
                    MenuItem::new("properties", "属性")
                        .enable(!navigation::is_shell_folder(&self.dir())),
                    MenuItem::new("system-menu", "显示完整系统菜单")
                        .enable(!navigation::is_shell_folder(&self.dir())),
                ]
            }
            "access-menu" => {
                let path = self
                    .selected
                    .iter()
                    .next()
                    .and_then(|index| self.entries.get(*index))
                    .map(|entry| entry.path.clone())
                    .unwrap_or_else(|| self.dir());
                if path.has_root() && path.parent().is_none() {
                    // Explorer exposes only Advanced Sharing at a drive root.
                    vec![MenuItem::new("advanced-share", "高级共享…")]
                } else {
                    vec![
                        MenuItem::new("remove-access", "删除访问权限（系统设置）…"),
                        MenuItem::new("grant-access", "特定用户…"),
                    ]
                }
            }
            "background-menu" => vec![
                MenuItem::new("undo", "撤销    Ctrl+Z")
                    .enable(!self.undo_stack.is_empty() && !self.busy),
                MenuItem::new("redo", "重做    Ctrl+Y")
                    .enable(!self.redo_stack.is_empty() && !self.busy),
                MenuItem::separator(),
                MenuItem::new("submenu:view-menu", "查看                         ›"),
                MenuItem::new("submenu:sort-menu", "排序                         ›"),
                MenuItem::new("submenu:group", "分组依据                     ›"),
                MenuItem::new("refresh-folder", "刷新"),
                MenuItem::separator(),
                MenuItem::new("file-paste", "粘贴").enable(self.can_paste()),
                MenuItem::new("submenu:new-menu", "新建                         ›")
                    .enable(!navigation::is_virtual(&self.dir())),
                MenuItem::new("submenu:access-menu", "授予访问权限                 ›")
                    .enable(!navigation::is_virtual(&self.dir())),
                MenuItem::separator(),
                MenuItem::new("properties", "属性")
                    .enable(!navigation::is_shell_folder(&self.dir())),
                MenuItem::new("system-menu", "显示完整系统菜单")
                    .enable(!navigation::is_shell_folder(&self.dir())),
            ],
            "quick-background-menu" => vec![
                MenuItem::new("submenu:view-menu", "查看                         ›"),
                MenuItem::new("refresh-quick", "刷新"),
                MenuItem::separator(),
                MenuItem::new("file-paste", "粘贴").enable(false),
            ],
            "view-menu" => {
                let mut items: Vec<_> = ViewMode::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, (mode, label))| {
                        MenuItem::new(format!("view:{i}"), *label).check(self.view == *mode)
                    })
                    .collect();
                items.extend([
                    MenuItem::separator(),
                    MenuItem::new("details-pane", "详细信息窗格").check(self.settings.details_pane),
                    MenuItem::new("preview-pane", "预览窗格").check(self.preview_open),
                    MenuItem::separator(),
                    MenuItem::new("submenu:show", "显示                         ›"),
                ]);
                items
            }
            "show" => vec![
                MenuItem::new("checkboxes", "项目复选框").check(self.settings.checkboxes),
                MenuItem::new("extensions", "文件扩展名").check(self.settings.extensions),
                MenuItem::new("hidden", "隐藏的项目").check(self.hidden),
                MenuItem::new("compact", "紧凑视图").check(self.settings.compact),
            ],
            "nav-menu" => vec![
                MenuItem::new("nav-computer", "显示此电脑").check(self.settings.show_computer),
                MenuItem::new("nav-network", "显示网络").check(self.settings.show_network),
                MenuItem::new("nav-libraries", "显示库").check(self.settings.show_libraries),
                MenuItem::new("nav-all", "显示所有文件夹").check(self.settings.show_all_folders),
                MenuItem::new("nav-expand", "展开到当前文件夹").check(self.settings.expand_current),
            ],
            "quick-menu" | "tree-menu" => {
                let mut items = vec![
                    MenuItem::new("nav-tab", "在新标签页中打开"),
                    MenuItem::new("nav-window", "在新窗口中打开"),
                ];
                if kind == "quick-menu" {
                    let known_pinned = self.context_target.as_ref().is_some_and(|path| {
                        self.settings.favorites.contains(path)
                            || self
                                .system_quick_access
                                .iter()
                                .any(|item| &item.path == path && item.pinned == Some(true))
                    });
                    items.push(MenuItem::new(
                        "nav-unpin",
                        if known_pinned {
                            "从快速访问取消固定"
                        } else {
                            "从快速访问移除"
                        },
                    ));
                } else if self
                    .context_target
                    .as_ref()
                    .is_some_and(|p| !navigation::is_virtual(p))
                {
                    items.push(MenuItem::new("nav-pin", "固定到快速访问"));
                }
                let shell_target = self
                    .context_target
                    .as_ref()
                    .is_some_and(|path| navigation::is_shell_folder(path));
                items.extend([
                    MenuItem::separator(),
                    MenuItem::new("nav-copy-path", "复制文件地址"),
                    MenuItem::new("nav-properties", "属性").enable(!shell_target),
                    MenuItem::new("nav-system-menu", "显示完整系统菜单").enable(!shell_target),
                ]);
                items
            }
            "computer-menu" => vec![
                MenuItem::new("submenu:view-menu", "查看                         ›"),
                MenuItem::new("submenu:sort-menu", "排序方式                     ›"),
                MenuItem::new("submenu:group", "分组依据                     ›"),
                MenuItem::separator(),
                MenuItem::new("refresh-devices", "刷新"),
                MenuItem::new("computer-properties", "属性"),
            ],
            "sort-menu" => vec![
                MenuItem::new("sort:name", "名称").check(self.sort == Sort::Name),
                MenuItem::new("sort:modified", "修改日期").check(self.sort == Sort::Modified),
                MenuItem::new("sort:kind", "类型").check(self.sort == Sort::Kind),
                MenuItem::new(
                    "sort:size",
                    if self.usage_enabled {
                        "统计大小"
                    } else {
                        "大小"
                    },
                )
                .check(self.sort == Sort::Size),
                MenuItem::new("sort:created", "创建日期").check(self.sort == Sort::Created),
                MenuItem::separator(),
                MenuItem::new("ascending", "递增").check(!self.descending),
                MenuItem::new("descending", "递减").check(self.descending),
                MenuItem::separator(),
                MenuItem::new("submenu:group", "分组依据                     ›"),
            ],
            "group" => [
                ("", "（无）"),
                ("name", "名称"),
                ("modified", "修改日期"),
                ("kind", "类型"),
                ("size", "大小"),
                ("created", "创建日期"),
            ]
            .iter()
            .map(|(id, label)| {
                MenuItem::new(format!("group:{id}"), *label).check(self.group == *id)
            })
            .collect(),
            "columns" => if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
                vec!["name", "original", "deleted", "size", "kind", "modified"]
            } else {
                vec!["name", "modified", "kind", "size", "created"]
            }
            .iter()
            .map(|id| {
                MenuItem::new(format!("column:{id}"), column_label(id, self.usage_enabled))
                    .check(self.column_widths(0.).iter().any(|(key, _)| key == id))
                    .enable(*id != "name")
            })
            .chain([
                MenuItem::separator(),
                MenuItem::new("reset-columns", "重置列宽和顺序"),
            ])
            .collect(),
            "new-menu" => {
                let mut items = vec![
                    MenuItem::new("new-folder", "文件夹"),
                    MenuItem::new("new-shortcut", "快捷方式"),
                    MenuItem::separator(),
                ];
                items.extend(
                    self.templates
                        .iter()
                        .enumerate()
                        .map(|(i, t)| MenuItem::new(format!("template:{i}"), t.name.clone())),
                );
                items
            }
            "more-menu" => {
                let any = !self.selected.is_empty();
                let all_folders = any && self.selected.iter().all(|i| self.entries[*i].directory);
                let mut items = Vec::new();
                for id in &self.toolbar_overflow {
                    let item = match *id {
                        "view-menu" => {
                            MenuItem::new("submenu:view-menu", "查看                         ›")
                        }
                        "sort-menu" => {
                            MenuItem::new("submenu:sort-menu", "排序                         ›")
                        }
                        "cut" => MenuItem::new("file-cut", "剪切").enable(any),
                        "copy" => MenuItem::new("file-copy", "复制").enable(any),
                        "paste" => MenuItem::new("file-paste", "粘贴")
                            .enable(!navigation::is_app_virtual(&self.dir())),
                        "rename" => MenuItem::new("file-rename", "重命名").enable(
                            self.selected.len() == 1
                                && (self
                                    .selected
                                    .first()
                                    .and_then(|index| self.entries.get(*index))
                                    .is_some_and(|entry| {
                                        rust_explorer::volume_label::is_drive_root(&entry.path)
                                    })
                                    || (!self.is_home()
                                        && !navigation::is_shell_folder(&self.dir()))),
                        ),
                        "share" => MenuItem::new("file-share", "共享").enable(
                            self.selected.len() == 1
                                && !navigation::is_shell_folder(&self.dir())
                                && self
                                    .selected
                                    .first()
                                    .is_some_and(|i| !self.entries[*i].directory),
                        ),
                        "delete" => MenuItem::new("file-delete", "删除").enable(any),
                        "usage" => MenuItem::new(
                            "usage",
                            if self.usage_running {
                                "停止统计大小"
                            } else {
                                "统计大小"
                            },
                        ),
                        "global" => MenuItem::new("global", "在全盘搜索")
                            .check(self.tabs[self.active].global),
                        _ => continue,
                    };
                    items.push(item);
                }
                if !items.is_empty() {
                    items.push(MenuItem::separator());
                }
                items.extend([
                    MenuItem::new("zip", "压缩为 ZIP 文件").enable(any),
                    MenuItem::new("undo", "撤销    Ctrl+Z")
                        .enable(!self.undo_stack.is_empty() && !self.busy),
                    MenuItem::new("redo", "重做    Ctrl+Y")
                        .enable(!self.redo_stack.is_empty() && !self.busy),
                    MenuItem::new(
                        "pin",
                        if all_folders {
                            "固定到快速访问"
                        } else {
                            "添加到收藏夹"
                        },
                    )
                    .enable(any),
                    MenuItem::new("copy-path", "复制路径").enable(any),
                    MenuItem::separator(),
                    MenuItem::new("select-all", "全部选择"),
                    MenuItem::new("select-none", "全部取消").enable(any),
                    MenuItem::new("select-invert", "反向选择"),
                    MenuItem::separator(),
                    MenuItem::new("properties", "属性"),
                    MenuItem::new("options", "选项"),
                    MenuItem::separator(),
                    MenuItem::new("new-window", "新建窗口"),
                    MenuItem::new("detach-tab", "将当前标签移至新窗口"),
                ]);
                items
            }
            _ => vec![],
        }
    }
    fn menu_panel(
        &self,
        kind: &str,
        x: f32,
        y: f32,
        level: usize,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let mut offset = 8.;
        let item_height = if kind.starts_with("filter-menu:") {
            rust_explorer::layout_metrics::COMPACT_FILE_ROW
        } else {
            34.
        };
        let mut panel = div()
            .id(("popup-panel", level))
            // A submenu can overlap an earlier panel after flipping at a window edge.
            // Its hitbox must block the obscured rows' hover and click handlers too.
            .occlude()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(ui(260.))
            .max_h(px((viewport_height - y - 8.).max(1.).min(580. * self.zoom)))
            .overflow_y_scroll()
            .py_2()
            .px_1()
            .rounded(ui(8.))
            .bg(rgb(if t.dark { 0x2b2b2b } else { 0xfafafa }))
            .border_1()
            .border_color(rgb(t.border))
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for (index, item) in self.menu_items(kind, cx).into_iter().enumerate() {
            if item.id == "-" {
                panel = panel.child(div().h(ui(1.)).flex_shrink_0().my_1().bg(rgb(t.border)));
                offset += 9.;
                continue;
            }
            let enabled = item.enabled;
            let id = item.id;
            let hover_id = id.clone();
            let item_offset = offset;
            offset += item_height;
            let icon = menu_icon(&id);
            panel = panel.child(
                div()
                    .id((
                        if level == 0 {
                            "menu-item"
                        } else {
                            "submenu-item"
                        },
                        index,
                    ))
                    .h(ui(item_height))
                    .flex_shrink_0()
                    .px_2()
                    .rounded(ui(4.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(move |s| s.bg(rgb(if t.dark { 0x414141 } else { t.hover })))
                    })
                    .when(!enabled, |d| d.text_color(rgb(t.muted)).opacity(0.45))
                    .child(
                        div()
                            .w(ui(16.))
                            .flex_shrink_0()
                            .text_color(rgb(t.accent))
                            .when(item.checked, |d| d.child(visual::icon("check", t.accent))),
                    )
                    .when_some(icon, |d, icon| d.child(visual::icon(icon, t.text)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(kind.starts_with("filter-menu:"), |d| {
                                d.whitespace_nowrap().overflow_hidden().text_ellipsis()
                            })
                            .child(item.label.trim_end_matches('›').trim().to_string()),
                    )
                    .when(id.starts_with("submenu:"), |d| {
                        d.child(visual::icon("chevron-right", t.muted))
                    })
                    .on_mouse_move(cx.listener(move |v, _, _, cx| {
                        if level <= 1 {
                            let next = hover_id
                                .strip_prefix("submenu:")
                                .map(|kind| (kind.to_string(), item_offset));
                            if level == 0 && v.submenu != next {
                                v.submenu = next;
                                v.nested_submenu = None;
                                cx.notify();
                            } else if level == 1 && v.nested_submenu != next {
                                v.nested_submenu = next;
                                cx.notify();
                            }
                        }
                    }))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        cx.stop_propagation();
                        if enabled {
                            if let Some(kind) = id.strip_prefix("submenu:") {
                                if level == 0 {
                                    v.submenu = Some((kind.into(), item_offset));
                                    v.nested_submenu = None;
                                } else {
                                    v.nested_submenu = Some((kind.into(), item_offset));
                                }
                                cx.notify();
                            } else {
                                v.menu_command(&id, w, cx);
                            }
                        }
                    })),
            )
        }
        panel.into_any_element()
    }
    pub(super) fn render_popup(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (kind, position) = self.popup.clone().unwrap();
        if kind == "system-operations" {
            let t = self.theme;
            let mut panel = div()
                .id("system-operation-panel")
                .w(ui(520.))
                .max_w(px((f32::from(window.viewport_size().width) - 24.).max(120.)))
                .max_h(px((f32::from(window.viewport_size().height) - 80.).max(80.)))
                .overflow_y_scroll()
                .p_3()
                .rounded(ui(8.))
                .bg(rgb(t.input))
                .border_1()
                .border_color(rgb(t.border))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().text_size(ui(15.)).child("系统操作详情"));
            if self.system_operations.is_empty() {
                panel = panel.child("暂无系统操作记录");
            }
            for op in self.system_operations.iter().rev() {
                let elapsed = op
                    .finished
                    .unwrap_or_else(Instant::now)
                    .duration_since(op.started)
                    .as_secs_f32();
                let id = op.id;
                let mut card = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_2()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(op.label.clone())
                    .child(format!("用时：{elapsed:.1} 秒"))
                    .child(div().whitespace_normal().child(op.status.clone()));
                if op.finished.is_none() {
                    card = card.child(div().flex().gap_2()
                        .child(button(("stop-operation", id), "停止", t).on_click(cx.listener(move |v, _, w, cx| v.menu_command(&format!("operation-stop:{id}"), w, cx))))
                        .child(button(("force-operation", id), "强行停止", t).on_click(cx.listener(move |v, _, w, cx| v.menu_command(&format!("operation-force:{id}"), w, cx)))))
                        .child(div().text_size(ui(11.)).text_color(rgb(t.muted)).child("强行停止不会回滚已完成的文件操作；提升权限的系统窗口可能需要手动关闭。"));
                }
                panel = panel.child(card);
            }
            return div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, _, _, cx| {
                        v.popup = None;
                        cx.notify();
                    }),
                )
                .child(panel)
                .into_any_element();
        }
        let width = 260. * self.zoom;
        let height = self
            .menu_items(&kind, cx)
            .iter()
            .map(|i| {
                if i.id == "-" {
                    9.
                } else if kind.starts_with("filter-menu:") {
                    rust_explorer::layout_metrics::COMPACT_FILE_ROW
                } else {
                    34.
                }
            })
            .sum::<f32>()
            * self.zoom
            + 16. * self.zoom;
        let max_x = f32::from(window.viewport_size().width) - width - 8.;
        let max_y = f32::from(window.viewport_size().height);
        let pointer_x = f32::from(position.x);
        let pointer_y = f32::from(position.y);
        let x = (if pointer_x > max_x {
            pointer_x - width
        } else {
            pointer_x
        })
        .clamp(8., max_x.max(8.));
        let y = (if pointer_y + height.min(580. * self.zoom) > max_y - 8. {
            pointer_y - height.min(580. * self.zoom)
        } else {
            pointer_y
        })
        .clamp(
            40. * self.zoom,
            (max_y - height.min(580. * self.zoom) - 8.).max(40. * self.zoom),
        );
        let mut overlay = div()
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    cx.stop_propagation();
                    v.popup = None;
                    v.submenu = None;
                    v.nested_submenu = None;
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, _, _, cx| {
                    cx.stop_propagation();
                    v.popup = None;
                    v.submenu = None;
                    v.nested_submenu = None;
                    cx.notify();
                }),
            )
            .child(self.menu_panel(&kind, x, y, 0, max_y, cx));
        if let Some((sub, offset)) = &self.submenu {
            if self
                .menu_items(&kind, cx)
                .iter()
                .any(|i| i.id == format!("submenu:{sub}"))
            {
                let child_height = (self.menu_items(sub, cx).len() as f32 * 34. + 16.) * self.zoom;
                let child_x = if x + width * 2. < f32::from(window.viewport_size().width) {
                    x + width - 2.
                } else {
                    (x - width + 2.).max(8.)
                };
                let child_y =
                    (y + offset * self.zoom).min((max_y - child_height - 8.).max(40. * self.zoom));
                overlay = overlay.child(self.menu_panel(sub, child_x, child_y, 1, max_y, cx));
                if let Some((nested, offset)) = &self.nested_submenu {
                    if self
                        .menu_items(sub, cx)
                        .iter()
                        .any(|i| i.id == format!("submenu:{nested}"))
                    {
                        let height = ((self.menu_items(nested, cx).len() as f32 * 34. + 16.)
                            * self.zoom)
                            .min(580. * self.zoom);
                        let nested_x =
                            if child_x + width * 2. < f32::from(window.viewport_size().width) {
                                child_x + width - 2.
                            } else {
                                (child_x - width + 2.).max(8.)
                            };
                        let nested_y = (child_y + offset * self.zoom)
                            .min((max_y - height - 8.).max(40. * self.zoom));
                        overlay = overlay
                            .child(self.menu_panel(nested, nested_x, nested_y, 2, max_y, cx));
                    }
                }
            }
        }
        overlay.into_any_element()
    }
    /// Drop every generated cache (memory, thumbnails, folder sizes, disk).
    /// Shared by the selection menu and the advanced options page.
    pub(super) fn clear_caches(&mut self, cx: &mut Context<Self>) {
        self.usage_cancel.store(true, Ordering::Relaxed);
        self.usage_revision += 1;
        self.usage_running = false;
        self.usage.clear();
        self.usage_enabled = false;
        disk_usage::invalidate_cache();
        visual::clear_cache();
        visual::reset_image_cache(cx);
        self.thumbnails = rust_explorer::thumbnails::Loader::new();
        self.thumbnail_images = visual::ThumbnailImages::new(cx);
        self.invalidate_list();
        self.sync_thumbnails();
        self.status = "正在清空缓存…".into();
        let task = cx
            .background_executor()
            .spawn(async { rust_explorer::native_assets::clear_disk_cache() });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                visual::clear_cache();
                visual::reset_image_cache(cx);
                v.thumbnails = rust_explorer::thumbnails::Loader::new();
                v.thumbnail_images = visual::ThumbnailImages::new(cx);
                v.status = match result {
                    Ok((removed, failed)) => format!(
                        "已清空缓存 · 删除 {removed} 个生成文件 · {failed} 个文件正在使用或无法删除"
                    ),
                    Err(error) => format!("内存缓存已清空，磁盘缓存清理失败：{error:#}"),
                };
                v.sync_thumbnails();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn menu_command(&mut self, id: &str, w: &mut Window, cx: &mut Context<Self>) {
        if let Some((force, number)) = id
            .strip_prefix("operation-stop:")
            .map(|id| (false, id))
            .or_else(|| id.strip_prefix("operation-force:").map(|id| (true, id)))
        {
            if let Ok(number) = number.parse::<u64>() {
                if let Some(op) = self
                    .system_operations
                    .iter_mut()
                    .find(|op| op.id == number && op.finished.is_none())
                {
                    op.control.stop(force);
                    op.status = if force {
                        "正在强行停止"
                    } else {
                        "正在请求停止"
                    }
                    .into();
                }
            }
            cx.notify();
            return;
        }
        if let Some(payload) = id.strip_prefix("shell-verb:") {
            match serde_json::from_str::<shell::Request>(payload) {
                Ok(request @ shell::Request::InvokeVerb { .. }) => self.run_shell(request, cx),
                _ => self.error = Some("无效的系统菜单操作".into()),
            }
            cx.notify();
            return;
        }
        if let Some(command) = id.strip_prefix("text:") {
            self.popup = None;
            if let Some(input) = self.text_menu_target.take() {
                input.update(cx, |input, cx| input.text_command(command, w, cx));
            }
            cx.notify();
            return;
        }
        if id == "quick-pinned" || id == "quick-unpinned" {
            // The native menu already changed the Shell's Quick Access list; pull the
            // new state immediately instead of waiting for the periodic refresh.
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            self.quick_access_tick = Instant::now() - Duration::from_secs(16);
            self.refresh_quick_access(cx);
            self.status = if id == "quick-pinned" {
                "已固定到快速访问".into()
            } else {
                "已从快速访问取消固定".into()
            };
            w.focus(&self.focus);
            cx.notify();
            return;
        }
        if id == "filter-done" {
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            w.focus(&self.focus);
            cx.notify();
            return;
        }
        if id == "open-new-tab" || id == "open-new-window" {
            let mut folders: Vec<PathBuf> = self
                .selected
                .iter()
                .filter_map(|index| self.entries.get(*index))
                .filter(|entry| entry.directory && !entry.link)
                .map(|entry| entry.path.clone())
                .collect();
            // Drives and other tiles are not always part of the selection when the
            // menu was opened on them; fall back to the menu's own target.
            if folders.is_empty() {
                if let Some(target) = self.context_target.clone() {
                    folders.push(target);
                }
            }
            shell::menu_trace(format!(
                "menu: {id} for {} folder(s): {:?}",
                folders.len(),
                folders
            ));
            // No presentation is copied from this window: a folder is always listed
            // the way the settings resolve it (saved per-folder view, else the
            // built-in default). See `resolved_view` in ui.rs.
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            w.focus(&self.focus);
            for path in folders {
                if id == "open-new-tab" {
                    self.open_tab_path(path, cx);
                } else {
                    let tab = Tab {
                        id: next_tab_id(),
                        history: History::new(path),
                        query: String::new(),
                        global: false,
                        reveal_targets: Vec::new(),
                    };
                    let _ = open_explorer(Some(tab), cx);
                }
            }
            cx.notify();
            return;
        }
        if let Some((column, option)) = id
            .strip_prefix("filter-option:")
            .and_then(|s| s.split_once(':'))
        {
            self.filters.toggle(column, option);
            self.rebuild_rows();
            self.clear_preview();
            cx.notify();
            return;
        }
        if let Some(column) = id.strip_prefix("filter-clear:") {
            self.filters.columns.remove(column);
            self.rebuild_rows();
            cx.notify();
            return;
        }
        if id == "filter-reset" {
            self.filters = Default::default();
            self.rebuild_rows();
            cx.notify();
            return;
        }
        if let Some(action) = id.strip_prefix("drop:") {
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            if let Some((paths, destination)) = self.pending_right_drop.take() {
                let mode = match action {
                    "copy" => Some(shell::TransferMode::Copy),
                    "move" => Some(shell::TransferMode::Move),
                    "link" => Some(shell::TransferMode::Link),
                    _ => None,
                };
                if let Some(mode) = mode {
                    self.confirm_drop_mode(paths, destination, mode, cx);
                }
            }
            cx.notify();
            return;
        }
        if let Some(index) = id
            .strip_prefix("hidden-crumb:")
            .and_then(|id| id.parse::<usize>().ok())
        {
            let path = self
                .hidden_breadcrumbs
                .borrow()
                .get(index)
                .map(|(_, path)| path.clone());
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            if let Some(path) = path {
                let child = self.child_towards_current(&path);
                self.navigate(path, cx);
                if let Some(child) = child {
                    self.focus_after_load(child);
                }
            }
            return;
        }
        if let Some(id) = id.strip_prefix("crumb:") {
            let path = if id == "computer" {
                Some(volumes::computer())
            } else {
                id.parse::<usize>().ok().and_then(|i| {
                    self.dir()
                        .ancestors()
                        .skip(1)
                        .nth(i)
                        .map(|p| p.to_path_buf())
                })
            };
            self.popup = None;
            self.submenu = None;
            self.nested_submenu = None;
            if let Some(path) = path {
                let child = self.child_towards_current(&path);
                self.navigate(path, cx);
                if let Some(child) = child {
                    self.focus_after_load(child);
                }
            }
            return;
        }
        if let Some(kind) = id.strip_prefix("submenu:") {
            self.submenu = Some((kind.into(), 0.));
            cx.notify();
            return;
        }
        self.popup = None;
        self.submenu = None;
        self.nested_submenu = None;
        if let Some(index) = id
            .strip_prefix("view:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some((view, _)) = ViewMode::ALL.get(index) {
                self.view = *view;
                self.content_scroll.set_offset(point(px(0.), px(0.)));
                self.high_performance_row.set(0.);
                self.list_scroll.scroll_to(ListOffset::default());
                self.settings.view = *view;
                if self.settings.remember_views {
                    self.settings.folder_views.insert(self.dir(), *view);
                }
                self.rebuild_rows();
                self.save_settings(cx);
            }
            cx.notify();
            return;
        }
        if let Some(id) = id.strip_prefix("sort:") {
            self.sort_current(column_sort(id), self.descending);
            cx.notify();
            return;
        }
        if let Some(id) = id.strip_prefix("group:") {
            self.group = id.into();
            self.rebuild_rows();
            cx.notify();
            return;
        }
        if let Some(id) = id.strip_prefix("column:") {
            if let Some(index) = self
                .active_columns_mut()
                .iter()
                .position(|(key, _)| key == id)
            {
                if id != "name" {
                    self.active_columns_mut().remove(index);
                }
            } else {
                self.active_columns_mut()
                    .push((id.into(), if id == "created" { 156. } else { 110. }));
            }
            self.invalidate_list();
            self.save_settings(cx);
            cx.notify();
            return;
        }
        if let Some(index) = id
            .strip_prefix("template:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some(template) = self.templates.get(index).cloned() {
                let folder = self.dir();
                self.busy = true;
                let task = cx
                    .background_executor()
                    .spawn(async move { new_file::create(&folder, &template) });
                cx.spawn(async move |view, cx| {
                    let result = task.await;
                    let _ = view.update(cx, |v, cx| {
                        v.busy = false;
                        match result {
                            Ok(path) => {
                                v.pending_select = Some(path.clone());
                                v.run_shell(shell::Request::RecordCreated { path }, cx);
                            }
                            Err(e) => {
                                v.error = Some(e.to_string());
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            return;
        }
        match id {
            "undo" => self.replay_history(false, cx),
            "redo" => self.replay_history(true, cx),
            "file-open" => self.open_selected(cx),
            "file-share" => self.run_shell(
                shell::Request::Share {
                    paths: self.paths(),
                },
                cx,
            ),
            "usage" => self.start_usage(cx),
            "global" => {
                self.tabs[self.active].global = !self.tabs[self.active].global;
                self.sync_search_placeholder(cx);
                if !self.tabs[self.active].query.is_empty() {
                    self.reload(cx);
                }
            }
            "file-explorer" => {
                for path in self.paths() {
                    self.run_shell(shell::Request::Open { path }, cx);
                }
            }
            "file-tab" => {
                if let Some(path) = self.paths().first() {
                    self.tabs.push(Tab {
                        id: next_tab_id(),
                        history: History::new(path.clone()),
                        query: String::new(),
                        global: false,
                        reveal_targets: Vec::new(),
                    });
                    self.switch_tab(self.tabs.len() - 1, cx);
                }
            }
            "file-window" => {
                let folders: Vec<PathBuf> = self
                    .selected
                    .iter()
                    .filter_map(|index| self.entries.get(*index))
                    .filter(|entry| entry.directory)
                    .map(|entry| entry.path.clone())
                    .collect();
                for path in folders {
                    let tab = Tab {
                        id: next_tab_id(),
                        history: History::new(path),
                        query: String::new(),
                        global: false,
                        reveal_targets: Vec::new(),
                    };
                    let _ = open_explorer(Some(tab), cx);
                }
            }
            "file-cut" => self.copy(true, cx),
            "file-copy" => self.copy(false, cx),
            "file-paste" | "paste" => self.paste(cx),
            "file-rename" => {
                // The app popup is only used by the list (and the context menu).
                self.pending_rename_site = Some(RenameSite::List);
                self.rename_dialog(w, cx)
            }
            "file-delete" => {
                if w.modifiers().shift {
                    let paths = self.paths();
                    if !paths.is_empty() {
                        self.show_dialog(Dialog::PermanentDelete(paths), w, cx);
                    }
                } else {
                    self.delete_dialog(w, cx);
                }
            }
            "system-menu" => self.system_menu(cx),
            "grant-access" | "remove-access" | "advanced-share" => {
                let path = self
                    .paths()
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| self.dir());
                if !navigation::is_virtual(&path) {
                    if id == "advanced-share" {
                        self.run_shell(shell::Request::AdvancedShare { path }, cx);
                    } else {
                        self.run_shell(
                            shell::Request::SharingAccess {
                                path,
                                remove: id == "remove-access",
                            },
                            cx,
                        );
                    }
                }
            }
            "refresh-folder" => self.reload(cx),
            "refresh-quick" => {
                self.quick_access_tick = Instant::now() - Duration::from_secs(30);
                self.refresh_quick_access(cx);
                self.sync_computer_entries();
            }
            "nav-computer" => self.settings.show_computer = !self.settings.show_computer,
            "nav-network" => self.settings.show_network = !self.settings.show_network,
            "nav-libraries" => self.settings.show_libraries = !self.settings.show_libraries,
            "nav-all" => self.settings.show_all_folders = !self.settings.show_all_folders,
            "nav-expand" => {
                self.settings.expand_current = !self.settings.expand_current;
                if self.settings.expand_current {
                    self.expand_to_current(cx);
                }
            }
            "nav-unpin" => {
                if let Some(path) = self.context_target.clone() {
                    self.change_quick_access(vec![path], false, cx);
                }
            }
            "nav-pin" => {
                if let Some(path) = self.context_target.clone() {
                    self.change_quick_access(vec![path], true, cx);
                }
            }
            "nav-tab" => {
                if let Some(path) = self.context_target.clone() {
                    self.tabs.push(Tab {
                        id: next_tab_id(),
                        history: History::new(path),
                        query: String::new(),
                        global: false,
                        reveal_targets: Vec::new(),
                    });
                    self.switch_tab(self.tabs.len() - 1, cx);
                }
            }
            "nav-window" => {
                if let Some(path) = self.context_target.clone() {
                    let _ = open_explorer(
                        Some(Tab {
                            id: next_tab_id(),
                            history: History::new(path),
                            query: String::new(),
                            global: false,
                            reveal_targets: Vec::new(),
                        }),
                        cx,
                    );
                }
            }
            "nav-copy-path" => {
                if let Some(path) = &self.context_target {
                    cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                }
            }
            "nav-properties" => {
                if let Some(path) = self.context_target.clone() {
                    if navigation::is_virtual(&path) {
                        self.run_shell(shell::Request::SystemProperties, cx);
                    } else {
                        self.run_shell(shell::Request::Properties { path }, cx);
                    }
                }
            }
            "nav-system-menu" => {
                if let Some(path) = self.context_target.clone() {
                    if !navigation::is_virtual(&path) {
                        let (x, y) = shell::cursor();
                        let quick_access_pinned = self
                            .system_quick_access
                            .iter()
                            .any(|item| item.path == path && item.pinned == Some(true));
                        self.run_shell(
                            shell::Request::ThemedMenu {
                                folder: path.parent().unwrap_or(&path).to_path_buf(),
                                paths: vec![path],
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
                                    Sort::Created => "created",
                                    Sort::Original => "original",
                                    Sort::Deleted => "deleted",
                                    Sort::Kind => "kind",
                                    Sort::Size => "size",
                                }
                                .into(),
                                descending: self.descending,
                                group: self.group.clone(),
                                undo: !self.undo_stack.is_empty(),
                                redo: !self.redo_stack.is_empty(),
                                can_paste: false,
                                quick_access_pinned,
                                owner: shell::active_window_owner(),
                            },
                            cx,
                        );
                    }
                }
            }
            "refresh-devices" => self.refresh_volumes(cx),
            "computer-open" => self.navigate(volumes::computer(), cx),
            "computer-expand" => self.computer_open = !self.computer_open,
            "computer-tab" | "computer-window" | "computer-pin" => {
                self.context_target = Some(volumes::computer());
                self.menu_command(
                    match id {
                        "computer-tab" => "nav-tab",
                        "computer-window" => "nav-window",
                        _ => "nav-pin",
                    },
                    w,
                    cx,
                );
            }
            "computer-properties" => self.run_shell(shell::Request::SystemProperties, cx),
            "computer-rename" => {
                self.context_target = Some(volumes::computer());
                self.begin_inline_rename(volumes::computer(), RenameSite::Tree, w, cx);
            }
            "new-folder" => self.show_dialog(Dialog::NewFolder, w, cx),
            "new-shortcut" => self.show_dialog(Dialog::Shortcut, w, cx),
            "checkboxes" => self.settings.checkboxes = !self.settings.checkboxes,
            "extensions" => self.settings.extensions = !self.settings.extensions,
            "hidden" => {
                self.hidden = !self.hidden;
                self.settings.hidden = self.hidden;
                self.reload(cx);
            }
            "compact" => self.settings.compact = !self.settings.compact,
            "wrap" => self.settings.wrap_names = !self.settings.wrap_names,
            "preview-pane" => {
                self.preview_open = !self.preview_open;
                self.settings.preview = self.preview_open;
                if self.preview_open {
                    self.settings.details_pane = false;
                }
                self.load_preview(cx);
            }
            "details-pane" => {
                self.settings.details_pane = !self.settings.details_pane;
                if self.settings.details_pane {
                    self.preview_open = false;
                    self.settings.preview = false;
                }
                self.load_preview(cx);
            }
            "ascending" => self.sort_current(self.sort, false),
            "descending" => self.sort_current(self.sort, true),
            "reset-columns" => {
                let defaults = if rust_explorer::shell_folder::is_recycle_bin_path(&self.dir()) {
                    rust_explorer::settings::default_recycle_columns()
                } else {
                    Settings::default().columns
                };
                *self.active_columns_mut() = defaults;
            }
            "select-all" => {
                self.selected = self.visible_file_indices();
                self.clear_preview();
            }
            "select-none" => {
                self.selected.clear();
                self.clear_preview();
            }
            "select-invert" => {
                self.selected = self
                    .visible_file_indices()
                    .into_iter()
                    .filter(|i| !self.selected.contains(i))
                    .collect();
                self.load_preview(cx);
            }
            "copy-path" => {
                let text = self
                    .paths()
                    .iter()
                    .map(|p| format!("\"{}\"", p.display()))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = "已复制路径".into();
            }
            "properties" => {
                let mut paths = self.paths();
                if paths.is_empty() {
                    if navigation::is_app_virtual(&self.dir()) {
                        self.run_shell(shell::Request::SystemProperties, cx);
                        return;
                    }
                    paths.push(self.dir());
                }
                if paths.len() == 1 && rust_explorer::shell_folder::is_shell_path(&paths[0]) {
                    self.run_shell(
                        shell::Request::Properties {
                            path: paths.remove(0),
                        },
                        cx,
                    );
                } else {
                    self.run_shell(shell::Request::PropertiesMany { paths }, cx);
                }
            }
            "pin" => {
                self.change_quick_access(self.paths(), true, cx);
            }
            "options" => {
                self.options = Some(self.settings.clone());
                self.options_page = 0;
            }
            "new-window" => {
                let tab = Tab {
                    id: next_tab_id(),
                    history: History::new(self.dir()),
                    query: String::new(),
                    global: false,
                    reveal_targets: Vec::new(),
                };
                let _ = open_explorer(Some(tab), cx);
            }
            "detach-tab" => self.detach_tab(self.tabs[self.active].id, cx),
            "zip" => self.compress_selection(cx),
            _ => {}
        }
        self.invalidate_list();
        self.save_settings(cx);
        cx.notify();
    }
    fn compress_selection(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths();
        if paths.is_empty() || self.busy {
            return;
        }
        let folder = self.dir();
        self.busy = true;
        self.status = "正在压缩 ZIP…".into();
        let task = cx.background_executor().spawn(async move {
            (move || -> anyhow::Result<PathBuf> {
                let stem = if paths.len() == 1 {
                    paths[0]
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                } else {
                    "存档".into()
                };
                let archive = (0..10000)
                    .map(|i| {
                        folder.join(if i == 0 {
                            format!("{stem}.zip")
                        } else {
                            format!("{stem} ({i}).zip")
                        })
                    })
                    .find(|p| !p.exists())
                    .ok_or_else(|| anyhow::anyhow!("无法生成存档名称"))?;
                let system = std::env::var_os("WINDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
                let mut cmd = std::process::Command::new(system.join("System32/tar.exe"));
                cmd.args(["-a", "-cf"]).arg(&archive);
                for path in paths {
                    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                        cmd.arg("-C").arg(parent).arg(PathBuf::from(".").join(name));
                    }
                }
                use std::os::windows::process::CommandExt;
                let output = cmd.creation_flags(0x08000000).output()?;
                if !output.status.success() {
                    anyhow::bail!("压缩失败：{}", String::from_utf8_lossy(&output.stderr));
                }
                Ok(archive)
            })()
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(path) => {
                        v.pending_select = Some(path);
                        v.reload(cx);
                    }
                    Err(e) => {
                        v.error = Some(e.to_string());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn apply_options(&mut self, close: bool, cx: &mut Context<Self>) {
        if let Some(settings) = self.options.clone() {
            let view = if settings.view != self.settings.view {
                settings.view
            } else {
                self.view
            };
            self.settings = settings;
            self.theme = Theme::preference(&self.settings.theme);
            self.tree_children.clear();
            self.hidden = self.settings.hidden;
            self.preview_open = self.settings.preview;
            self.view = view;
            self.save_settings(cx);
            self.reload(cx);
            if close {
                self.options = None;
            }
        }
        cx.notify();
    }
    fn choose_resources(&mut self, directory: bool, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: !directory,
            directories: directory,
            multiple: false,
            prompt: Some("选择本机 Windows 界面资源".into()),
        });
        cx.spawn(async move |view, cx| {
            if let Ok(Ok(Some(paths))) = picker.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = view.update(cx, |v, cx| v.load_resources(Some(path), cx));
                }
            }
        })
        .detach();
    }
    fn load_resources(&mut self, source: Option<PathBuf>, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            rust_explorer::fluent_icons::set_resource_source(source).map_err(|e| e.to_string())
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                match result {
                    Ok(status) => {
                        v.status = format!(
                            "资源已更新：{} 个 SVG，{} 个已识别系统模块",
                            status.svg_count, status.native_module_count
                        )
                    }
                    Err(error) => v.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn change_folder_association(
        &mut self,
        enable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.association_busy {
            return;
        }
        self.association_busy = true;
        let hwnd = native_hwnd(window).unwrap_or(0);
        let task = cx.background_executor().spawn(async move {
            use windows::{core::PCWSTR, Win32::{Foundation::HWND, UI::WindowsAndMessaging::*}};
            let message = if enable {
                "将限时试用当前用户的普通文件夹和驱动器默认打开方式。Win+E、此电脑等虚拟页面保持系统行为。\n请先将程序放在固定位置，启用后不要移动或删除。\n未在 120 秒内确认保留，独立恢复进程会自动恢复原样，即使 RE 已退出。\n原始值及独立恢复程序位于本地应用数据目录 RustExplorer\\folder-association。\n是否开始试用？"
            } else { "恢复启用前的文件夹和驱动器关联？若关联已被其他程序修改，会保留这些修改并报告冲突。" };
            let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Rust Explorer — 文件夹关联".encode_utf16().chain(Some(0)).collect();
            let answer = unsafe { MessageBoxW(Some(HWND(hwnd as *mut _)), PCWSTR(message.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) };
            if answer != IDYES { return Ok(false); }
            if enable {
                rust_explorer::default_browser::enable().map_err(|e| format!("{e:#}"))?;
                if rust_explorer::default_browser::trial_pending() {
                    let keep: Vec<u16> = "临时接管已启用。请在 Windows 资源管理器中双击普通文件夹、驱动器，确认 RE 正常打开。\n是否保留此设置？\n选择“否”或 120 秒内未确认，会自动恢复原样。".encode_utf16().chain(Some(0)).collect();
                    let answer = unsafe { MessageBoxW(None, PCWSTR(keep.as_ptr()), PCWSTR(title.as_ptr()), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) };
                    if answer == IDYES { rust_explorer::default_browser::confirm_trial().map_err(|e| format!("{e:#}"))?; }
                    else if rust_explorer::default_browser::trial_pending() { rust_explorer::default_browser::disable().map_err(|e| format!("{e:#}"))?; return Ok(false); }
                    else { return Ok(false); }
                }
            } else { rust_explorer::default_browser::disable().map_err(|e| format!("{e:#}"))?; }
            Ok(true)
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.association_busy = false;
                match result {
                    Ok(true) => {
                        v.status = "文件夹关联操作完成；恢复入口位于文件夹选项 → 高级".into()
                    }
                    Ok(false) => {}
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// One checkbox row on the folder-options pages. The advanced page reuses
    /// it for the settings that are not part of a tab's main list.
    fn setting_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        div()
            .id(SharedString::from(format!("setting-{id}")))
            .min_h(ui(32.))
            .min_w_0()
            .w_full()
            .flex_shrink_0()
            .px_2()
            .py_1()
            .rounded(ui(4.))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(t.hover)))
            .flex()
            .items_center()
            .gap_2()
            .child(visual::checkbox(checked, t.muted))
            .child(div().flex_1().min_w_0().whitespace_normal().child(label))
            .on_click(cx.listener(move |v, _, _, cx| {
                if let Some(s) = &mut v.options {
                    toggle_option(s, id);
                }
                cx.notify();
            }))
            .into_any_element()
    }
    pub(super) fn render_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let settings = self.options.as_ref().unwrap();
        let mut tabs = div()
            .flex()
            .flex_shrink_0()
            .gap_1()
            .border_b_1()
            .border_color(rgb(t.border));
        for (index, label) in ["常规", "查看", "细节", "搜索", "高级"].iter().enumerate()
        {
            tabs = tabs.child(
                button(("option-tab", index), *label, t)
                    .when(self.options_page == index, |d| d.bg(rgb(t.selection)))
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.options_page = index;
                        cx.notify();
                    })),
            );
        }
        let mut body = div()
            .id("options-body")
            .track_scroll(&self.options_scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .flex_1()
            .w_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        if self.options_page == 0 {
            body = body.child(
                div()
                    .py_2()
                    .child(format!("当前软件版本：{}", env!("CARGO_PKG_VERSION"))),
            );
        }
        if self.options_page == 0 {
            let mut startup = div()
                .flex()
                .flex_wrap()
                .flex_shrink_0()
                .items_center()
                .gap_2();
            for (page, label) in [
                ("home", "主文件夹"),
                ("this-pc", "此电脑"),
                ("quick-access", "快速访问"),
            ] {
                let selected = settings.startup_page == page;
                startup = startup.child(
                    button(SharedString::from(format!("startup-page-{page}")), label, t)
                        .flex_shrink_0()
                        .when(selected, |d| d.bg(rgb(t.selection)))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if let Some(s) = &mut v.options {
                                s.startup_page = page.into();
                            }
                            cx.notify();
                        })),
                );
            }
            body = body.child(div().py_2().child("启动时打开")).child(startup);
        }
        let options: Vec<(&str, &str, bool)> = match self.options_page {
            0 => vec![
                (
                    "single",
                    "通过单击打开项目（指向时选定请使用复选框）",
                    settings.single_click,
                ),
                (
                    "separate",
                    "在新窗口中打开文件夹",
                    settings.separate_process,
                ),
                (
                    "remember",
                    "记住每个文件夹的查看方式",
                    settings.remember_views,
                ),
            ],
            1 => vec![
                (
                    "extensions",
                    "显示已知文件类型的扩展名",
                    settings.extensions,
                ),
                ("hidden", "显示隐藏的文件、文件夹和驱动器", settings.hidden),
                ("protected", "显示受保护的操作系统文件", settings.protected),
                ("checkboxes", "使用复选框以便选择项目", settings.checkboxes),
                (
                    "thumbnails",
                    "显示文件缩略图（中等图标及以上）",
                    settings.thumbnails,
                ),
                (
                    "compact",
                    "减小项目之间的空间（紧凑视图）",
                    settings.compact,
                ),
                (
                    "full-system-menu",
                    "右键直接显示完整系统菜单",
                    settings.full_system_menu,
                ),
                (
                    "full-path",
                    "在窗口标题栏和标签页中显示完整路径",
                    settings.full_path,
                ),
                (
                    "preview",
                    "显示预览窗格（文本、照片、视频帧、PDF）",
                    settings.preview,
                ),
            ],
            2 => vec![
                (
                    "quick-access-new-tab",
                    "默认在新标签页中打开快速访问的文件夹",
                    settings.quick_access_new_tab,
                ),
                (
                    "new-tab-this-pc",
                    "新标签页打开此电脑（关闭时打开当前目录）",
                    settings.new_tab_this_pc,
                ),
                (
                    "shift-open-tab",
                    "按住 Shift 打开文件夹时在新标签页中打开",
                    settings.shift_open_tab,
                ),
                (
                    "folders-first",
                    "始终将文件夹排列在文件前面",
                    settings.folders_first,
                ),
                (
                    "focus-existing-tab",
                    "在新标签页中打开文件夹时，若已有同一目录的标签页则直接切换过去",
                    settings.focus_existing_tab,
                ),
            ],
            _ => vec![],
        };
        if self.options_page == 0 {
            body = body.child(
                div()
                    .py_2()
                    .text_color(rgb(t.muted))
                    .child("浏览文件夹与打开项目"),
            );
        }
        if self.options_page == 1 {
            let mut themes = div()
                .flex()
                .flex_wrap()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .child("外观");
            for (id, label) in [("system", "跟随系统"), ("light", "亮色"), ("dark", "暗色")]
            {
                themes = themes.child(
                    button(SharedString::from(format!("theme-{id}")), label, t)
                        .when(settings.theme == id, |d| d.bg(rgb(t.selection)))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if let Some(s) = &mut v.options {
                                s.theme = id.into();
                            }
                            cx.notify();
                        })),
                );
            }
            body = body.child(themes);
        }
        if self.options_page == 2 {
            body = body.child(div().py_2().text_color(rgb(t.muted)).child("标签页与排序"));
        }
        if self.options_page == 4 {
            body = body
                .child(
                    div()
                        .py_2()
                        .text_color(rgb(t.muted))
                        .child("缓存与维护"),
                )
                .child(div().w_full().min_w_0().flex_shrink_0().whitespace_normal().text_color(rgb(t.muted)).child("清空图标、缩略图、文件夹大小和磁盘容量统计缓存；只删除本程序生成的文件。"))
                .child(div().flex().flex_wrap().flex_shrink_0().gap_2()
                    .child(button("options-clear-cache", "清空缓存", t).on_click(cx.listener(|v, _, _, cx| v.clear_caches(cx)))));
            body = body.child(
                div()
                    .py_2()
                    .text_color(rgb(t.muted))
                    .child("默认文件夹（实验性）"),
            );
            let status = rust_explorer::default_browser::status()
                .unwrap_or_else(|e| format!("关联状态读取失败：{e:#}"));
            body = body.child(div().w_full().min_w_0().flex_shrink_0().whitespace_normal().child(format!("默认文件夹浏览器（实验性）：{status}。仅普通文件夹和驱动器。先试用 120 秒，确认后保留，超时自动恢复；与下方“应用”独立。")))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("association-enable", if self.association_busy { "操作中…" } else { "试用默认文件夹浏览器…" }, t).on_click(cx.listener(|v, _, w, cx| v.change_folder_association(true, w, cx))))
                    .child(button("association-restore", "恢复原样…", t).on_click(cx.listener(|v, _, w, cx| v.change_folder_association(false, w, cx)))));
            body = body.child(
                button("association-recovery-folder", "打开恢复文件夹", t).on_click(cx.listener(
                    |v, _, _, cx| {
                        match rust_explorer::default_browser::recovery_directory().and_then(
                            |path| {
                                std::fs::create_dir_all(&path)?;
                                Ok(path)
                            },
                        ) {
                            Ok(path) => {
                                v.options = None;
                                v.open_tab_path(path, cx);
                            }
                            Err(e) => v.error = Some(e.to_string()),
                        }
                        cx.notify();
                    },
                )),
            );
            body = body
                .child(div().py_2().text_color(rgb(t.muted)).child("性能"))
                .child(self.setting_row(
                    "high-performance",
                    "高性能模式（一屏加缓冲；不适用于中等及以上图标）",
                    settings.high_performance,
                    cx,
                ));
            let source = rust_explorer::fluent_icons::resource_status();
            body = body
                .child(div().py_2().text_color(rgb(t.muted)).child("界面资源"))
                .child(div().w_full().min_w_0().flex_shrink_0().text_size(ui(12.)).whitespace_normal().child(format!(
                "界面资源：{}（SVG {}，系统资源模块 {}）。仅引用本机文件，不复制到程序目录。",
                source.source.map(|p| p.display().to_string()).unwrap_or_else(|| "当前系统".into()), source.svg_count, source.native_module_count
            ))).child(div().w_full().min_w_0().flex().flex_wrap().flex_shrink_0().gap_2()
                .child(button("resource-file", "选择 DLL / EXE / MUN", t).on_click(cx.listener(|v, _, _, cx| v.choose_resources(false, cx))))
                .child(button("resource-folder", "选择资源目录", t).on_click(cx.listener(|v, _, _, cx| v.choose_resources(true, cx))))
                .child(button("resource-reset", "使用当前系统", t).on_click(cx.listener(|v, _, _, cx| v.load_resources(None, cx)))));
        }
        for (id, label, checked) in options {
            body = body.child(self.setting_row(id, label, checked, cx));
        }
        if self.options_page == 1 {
            body = body.child(
                button("restore-views", "重置所有文件夹的查看设置", t)
                    .justify_start()
                    .on_click(cx.listener(|v, _, _, cx| {
                        if let Some(s) = &mut v.options {
                            s.folder_views.clear();
                            s.columns = Settings::default().columns;
                        }
                        cx.notify();
                    })),
            );
        }
        if self.options_page == 3 {
            body = body
                .child(div().w_full().min_w_0().flex_shrink_0().py_2().whitespace_normal().child("搜索引擎：Everything（已安装时自动调用）"))
                .child(div().w_full().min_w_0().flex_shrink_0().whitespace_normal().text_color(rgb(t.muted)).line_height(ui(24.)).child("支持 Everything 查询语法。搜索框旁可切换当前文件夹 / 全盘范围。索引器不可用时，只筛选当前文件夹名称。目录内容、压缩包内容及系统索引服务集成未启用，避免后台扫描。"));
        }
        div()
            .absolute()
            .inset_0()
            .occlude()
            .p_4()
            .on_mouse_move(cx.listener(|v, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    if let Some((handle, start, offset, ratio)) = &v.aux_scroll_drag {
                        let mut p = handle.offset();
                        p.y = px((*offset - (f32::from(event.position.y) - *start) * *ratio)
                            .clamp(-f32::from(handle.max_offset().height), 0.));
                        handle.set_offset(p);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .bg(rgba(0x00000070))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(ui(580.))
                    .min_w_0()
                    .max_w_full()
                    .h(ui(540.))
                    .max_h_full()
                    .overflow_hidden()
                    .p_5()
                    .rounded(ui(9.))
                    .bg(rgb(t.panel))
                    .border_1()
                    .border_color(rgb(t.border))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(ui(18.)).child("文件夹选项"))
                            .child(button("options-close", "×", t).on_click(cx.listener(
                                |v, _, _, cx| {
                                    v.options = None;
                                    cx.notify();
                                },
                            ))),
                    )
                    .child(tabs)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .flex()
                            .child(body.pr_4())
                            .child(self.auxiliary_scrollbar(
                                self.options_scroll.clone(),
                                "options-scrollbar",
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .whitespace_normal()
                            .text_size(ui(12.))
                            .text_color(rgb(t.muted))
                            .child("普通设置仅作用于 Rust Explorer；默认文件夹关联需单独确认，可通过“恢复原样”撤销。"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .flex_shrink_0()
                            .gap_2()
                            .justify_between()
                            .child(button("options-defaults", "还原默认值", t).on_click(
                                cx.listener(|v, _, _, cx| {
                                    v.options = Some(Settings::default());
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_2()
                                    .child(
                                        button("options-ok", "确定", t)
                                            .bg(rgb(t.selection))
                                            .on_click(
                                                cx.listener(|v, _, _, cx| {
                                                    v.apply_options(true, cx)
                                                }),
                                            ),
                                    )
                                    .child(button("options-cancel", "取消", t).on_click(
                                        cx.listener(|v, _, _, cx| {
                                            v.options = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(button("options-apply", "应用", t).on_click(
                                        cx.listener(|v, _, _, cx| v.apply_options(false, cx)),
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }
}
/// Flip one pending option; only applied when the dialog is confirmed.
fn toggle_option(settings: &mut Settings, id: &str) {
    match id {
        "quick-access-new-tab" => settings.quick_access_new_tab = !settings.quick_access_new_tab,
        "new-tab-this-pc" => settings.new_tab_this_pc = !settings.new_tab_this_pc,
        "single" => settings.single_click = !settings.single_click,
        "shift-open-tab" => settings.shift_open_tab = !settings.shift_open_tab,
        "separate" => settings.separate_process = !settings.separate_process,
        "remember" => settings.remember_views = !settings.remember_views,
        "folders-first" => settings.folders_first = !settings.folders_first,
        "focus-existing-tab" => settings.focus_existing_tab = !settings.focus_existing_tab,
        "extensions" => settings.extensions = !settings.extensions,
        "hidden" => settings.hidden = !settings.hidden,
        "protected" => settings.protected = !settings.protected,
        "checkboxes" => settings.checkboxes = !settings.checkboxes,
        "high-performance" => settings.high_performance = !settings.high_performance,
        "compact" => settings.compact = !settings.compact,
        "thumbnails" => settings.thumbnails = !settings.thumbnails,
        "full-system-menu" => settings.full_system_menu = !settings.full_system_menu,
        "full-path" => settings.full_path = !settings.full_path,
        "preview" => settings.preview = !settings.preview,
        _ => {}
    }
}
