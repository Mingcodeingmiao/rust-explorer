use super::*;
fn computer_name_cache() -> &'static std::sync::RwLock<String> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<String>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new("此电脑".into()))
}
pub(super) fn computer_name() -> String {
    computer_name_cache().read().unwrap().clone()
}
#[derive(Clone)]
struct NavRow {
    label: String,
    path: PathBuf,
    depth: usize,
    expandable: bool,
    pinned: bool,
    icon: String,
}
fn special(path: &std::path::Path) -> bool {
    path.to_string_lossy().starts_with("::")
}
pub(super) fn is_virtual(path: &std::path::Path) -> bool {
    special(path)
}
pub(super) fn location_name(path: &std::path::Path) -> String {
    match path.to_string_lossy().as_ref() {
        "::ThisPC" => computer_name(),
        "::Network" => "网络".into(),
        "::Libraries" => "库".into(),
        "::QuickAccess" => "快速访问".into(),
        _ => path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
    }
}
fn volume_type(volume: &Volume) -> &'static str {
    match volume.kind {
        2 => "可移动磁盘",
        4 => "网络驱动器",
        5 => "光盘驱动器",
        6 => "RAM 磁盘",
        _ => "本地磁盘",
    }
}
impl Explorer {
    pub(super) fn apply_computer_name(&mut self, name: &str, cx: &mut Context<Self>) {
        if name.is_empty() || name.contains('\n') || name.len() > 1024 {
            return;
        }
        self.computer_name = name.to_string();
        *computer_name_cache().write().unwrap() = self.computer_name.clone();
        if volumes::is_computer(&self.dir()) && !self.editing_address {
            let name = self.computer_name.clone();
            self.address
                .update(cx, |input, cx| input.set_value(name, cx));
        }
        cx.notify();
    }
    pub(super) fn refresh_computer_name(&mut self, cx: &mut Context<Self>) {
        let cached = computer_name();
        if self.computer_name != cached {
            self.computer_name = cached;
            cx.notify();
        }
        if self.computer_name_busy || self.computer_name_tick.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.computer_name_tick = Instant::now();
        self.computer_name_busy = true;
        let task = cx
            .background_executor()
            .spawn(async { shell::execute_output(shell::Request::ComputerName) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.computer_name_busy = false;
                if let Ok(value) = result {
                    if let Some(name) = value.strip_prefix("computer-name:") {
                        v.apply_computer_name(name.trim(), cx);
                    }
                }
            });
        })
        .detach();
    }
    fn navigation_anchor(&self) -> Option<(PathBuf, usize)> {
        self.nav_rows()
            .get(self.nav_keyboard_index)
            .map(|row| (row.path.clone(), row.depth))
    }
    fn restore_navigation_anchor(&mut self, anchor: Option<(PathBuf, usize)>) {
        if let Some((path, depth)) = anchor {
            self.nav_keyboard_index = self
                .nav_rows()
                .iter()
                .position(|row| row.path == path && row.depth == depth)
                .unwrap_or(usize::MAX);
        }
    }
    pub(super) fn navigation_context_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.context_target = Some(path.clone());
        self.popup = None;
        self.submenu = None;
        if volumes::is_computer(&path) {
            let (x, y) = shell::cursor();
            self.run_shell(
                shell::Request::ComputerMenu {
                    x,
                    y,
                    dark: self.theme.dark,
                    expanded: self.tree_is_open(&path),
                },
                cx,
            );
        } else if (self.settings.full_system_menu
            || rust_explorer::volume_label::is_drive_root(&path))
            && !special(&path)
            && !path.to_string_lossy().starts_with("shell:")
        {
            let (x, y) = shell::cursor();
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
                        Sort::Kind => "kind",
                        Sort::Size => "size",
                    }
                    .into(),
                    descending: self.descending,
                    group: self.group.clone(),
                    undo: !self.undo_stack.is_empty(),
                    redo: !self.redo_stack.is_empty(),
                },
                cx,
            );
        } else {
            let quick_access_entry =
                self.nav_rows()
                    .get(self.nav_keyboard_index)
                    .is_some_and(|row| {
                        row.path == path
                            && (row.pinned
                                || self
                                    .quick_access_items()
                                    .iter()
                                    .any(|(_, quick_path)| quick_path == &path))
                    });
            self.popup = Some((
                if quick_access_entry {
                    "quick-menu"
                } else {
                    "tree-menu"
                }
                .into(),
                position,
            ));
        }
        cx.notify();
    }
    pub(super) fn focus_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.nav_rows();
        if rows
            .get(self.nav_keyboard_index)
            .is_none_or(|row| row.path.as_os_str().is_empty())
        {
            self.nav_keyboard_index = rows
                .iter()
                .position(|row| row.path == self.dir())
                .or_else(|| rows.iter().position(|row| !row.path.as_os_str().is_empty()))
                .unwrap_or(0);
        }
        self.editing_address = false;
        self.keyboard_focus_region = 3;
        window.focus(&self.nav_focus);
        self.nav_scroll
            .scroll_to_item(self.nav_keyboard_index, ScrollStrategy::Center);
        cx.notify();
    }
    pub(super) fn navigate_tree(
        &mut self,
        command: rust_explorer::nav_keyboard::Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use rust_explorer::nav_keyboard::{self, Effect};
        if self.options.is_some() || self.dialog.is_some() {
            return;
        }
        let rows = self.nav_rows();
        let nodes: Vec<_> = rows
            .iter()
            .map(|row| nav_keyboard::Node {
                depth: row.depth,
                selectable: !row.path.as_os_str().is_empty(),
                expandable: row.expandable,
                open: self.tree_is_open(&row.path),
            })
            .collect();
        if command == nav_keyboard::Command::ContextMenu {
            if let Some(row) = rows
                .get(self.nav_keyboard_index)
                .filter(|row| !row.path.as_os_str().is_empty())
            {
                self.navigation_context_menu(row.path.clone(), point(px(24.), px(160.)), cx);
            }
            return;
        }
        match nav_keyboard::apply(&nodes, self.nav_keyboard_index, command) {
            Effect::Focus(index) => self.nav_keyboard_index = index,
            Effect::Expand(index) | Effect::Collapse(index) => {
                self.toggle_tree(rows[index].path.clone(), cx)
            }
            Effect::ExpandLevel(index) => {
                let path = rows[index].path.clone();
                if !special(&path) && !self.tree_children.contains_key(&path) {
                    self.nav_expand_pending = Some((path.clone(), rows[index].depth));
                }
                if !self.tree_is_open(&path) {
                    self.toggle_tree(path.clone(), cx);
                }
                self.expand_navigation_children(&path, rows[index].depth, cx);
            }
            Effect::Activate(index) => {
                let path = rows[index].path.clone();
                if self.settings.file_favorites.contains(&path)
                    || path == PathBuf::from("shell:RecycleBinFolder")
                {
                    self.run_shell(shell::Request::Open { path }, cx);
                } else {
                    self.navigate_with_modifiers(path, window.modifiers(), cx);
                }
            }
            Effect::None => {}
        }
        self.nav_scroll
            .scroll_to_item(self.nav_keyboard_index, ScrollStrategy::Center);
        window.focus(&self.nav_focus);
        cx.notify();
    }
    fn expand_navigation_children(
        &mut self,
        path: &std::path::Path,
        depth: usize,
        cx: &mut Context<Self>,
    ) {
        let rows = self.nav_rows();
        let Some(index) = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.path == path && row.depth == depth && row.expandable)
            .min_by_key(|(index, _)| index.abs_diff(self.nav_keyboard_index))
            .map(|(index, _)| index)
        else {
            return;
        };
        let nodes: Vec<_> = rows
            .iter()
            .map(|row| rust_explorer::nav_keyboard::Node {
                depth: row.depth,
                selectable: !row.path.as_os_str().is_empty(),
                expandable: row.expandable,
                open: self.tree_is_open(&row.path),
            })
            .collect();
        for index in rust_explorer::nav_keyboard::direct_children(&nodes, index, 16) {
            let child = rows[index].path.clone();
            if !self.tree_is_open(&child) {
                self.toggle_tree(child, cx);
            }
        }
    }
    pub(super) fn refresh_quick_access(&mut self, cx: &mut Context<Self>) {
        if self.quick_access_busy || self.quick_access_tick.elapsed() < Duration::from_secs(15) {
            return;
        }
        self.quick_access_busy = true;
        self.quick_access_tick = Instant::now();
        let task = cx.background_executor().spawn(async {
            volumes::bounded::<Vec<rust_explorer::quick_access::Item>>(
                "--quick-access",
                "null",
                Duration::from_millis(1600),
            )
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.quick_access_busy = false;
                if let Ok(items) = result {
                    if v.system_quick_access != items {
                        let anchor = v.navigation_anchor();
                        v.system_quick_access = items;
                        if v.dir() == PathBuf::from("::QuickAccess") && v.is_home() {
                            v.sync_computer_entries();
                        }
                        v.restore_navigation_anchor(anchor);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    pub(super) fn quick_access_items(&self) -> Vec<(String, PathBuf)> {
        let mut items: Vec<_> = self
            .system_quick_access
            .iter()
            .filter(|item| !self.settings.quick_access_hidden.contains(&item.path))
            .map(|item| (item.name.clone(), item.path.clone()))
            .collect();
        for path in &self.settings.favorites {
            if self.settings.quick_access_hidden.contains(path) {
                continue;
            }
            if items.iter().any(|(_, p)| p == path) {
                continue;
            }
            let label = self
                .places
                .iter()
                .find(|(_, p)| p == path)
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| {
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned()
                });
            items.push((label, path.clone()));
        }
        items
    }
    pub(super) fn refresh_volumes(&mut self, cx: &mut Context<Self>) {
        if self.volume_busy {
            return;
        }
        self.volume_busy = true;
        self.volume_tick = Instant::now();
        let previous: Vec<_> = self.volumes.iter().map(|v| v.path.clone()).collect();
        let capacity = self.volume_capacity_tick.elapsed() > Duration::from_secs(30);
        let task = cx.background_executor().spawn(async move {
            if !capacity && volumes::roots() == previous {
                None
            } else {
                Some(volumes::snapshot())
            }
        });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |v, cx| {
                v.volume_busy = false;
                let Some(volumes) = result else {
                    return;
                };
                v.volume_capacity_tick = Instant::now();
                cx.notify(); // Repaint even when capacity is unchanged so drive icon epochs refresh.
                if v.volumes != volumes {
                    let anchor = v.navigation_anchor();
                    v.volumes = volumes;
                    if v.is_home() {
                        v.sync_computer_entries();
                    }
                    v.restore_navigation_anchor(anchor);
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn sync_computer_entries(&mut self) {
        let selected = self.paths();
        let network = self.dir() == PathBuf::from("::Network");
        let mut entries: Vec<Entry> = if self.dir() == PathBuf::from("::QuickAccess") {
            self.quick_access_items()
                .into_iter()
                .map(|(name, path)| Entry {
                    path,
                    name,
                    directory: true,
                    hidden: false,
                    system: false,
                    link: false,
                    size: 0,
                    modified: None,
                    created: None,
                    kind: "文件夹".into(),
                })
                .collect()
        } else if self.dir() == PathBuf::from("::Libraries") {
            self.places
                .iter()
                .filter(|(n, _)| matches!(n.as_str(), "文档" | "图片" | "音乐" | "视频"))
                .map(|(label, path)| Entry {
                    path: path.clone(),
                    name: label.clone(),
                    directory: true,
                    hidden: false,
                    system: false,
                    link: false,
                    size: 0,
                    modified: None,
                    created: None,
                    kind: "库文件夹".into(),
                })
                .collect()
        } else {
            self.volumes
                .iter()
                .filter(|volume| !network || volume.kind == 4)
                .filter(|volume| {
                    self.tabs[self.active].query.is_empty()
                        || volume
                            .label
                            .to_lowercase()
                            .contains(&self.tabs[self.active].query.to_lowercase())
                })
                .map(|volume| Entry {
                    path: volume.path.clone(),
                    name: volume.label.clone(),
                    directory: true,
                    hidden: false,
                    system: false,
                    link: false,
                    size: volume.total.unwrap_or(0),
                    modified: None,
                    created: None,
                    kind: if volume.kind == 4 {
                        "网络位置"
                    } else {
                        "驱动器"
                    }
                    .into(),
                })
                .collect()
        };
        if volumes::is_computer(&self.dir()) || self.dir() == PathBuf::from("::Network") {
            entries.sort_by(|a, b| {
                match (
                    self.volumes.iter().find(|v| v.path == a.path),
                    self.volumes.iter().find(|v| v.path == b.path),
                ) {
                    (Some(a), Some(b)) => volumes::compare(
                        a,
                        b,
                        &self.settings.volume_sort,
                        self.settings.volume_descending,
                    ),
                    _ => a.path.cmp(&b.path),
                }
            });
        } else if self.dir() != PathBuf::from("::QuickAccess") {
            fs::sort(&mut entries, self.sort, self.descending);
        }
        self.entries = Arc::new(entries);
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| selected.contains(&e.path).then_some(i))
            .collect();
        self.status = format!("{} 个项目", self.entries.len());
        self.rebuild_rows();
    }
    fn tree_is_open(&self, path: &std::path::Path) -> bool {
        if volumes::is_computer(path) {
            self.computer_open
        } else if path == std::path::Path::new("::QuickAccess") {
            !self
                .tree_open
                .contains(&PathBuf::from("::QuickAccessCollapsed"))
        } else {
            self.tree_open.contains(path)
        }
    }
    pub(super) fn load_tree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.tree_loading.contains(&path) || self.tree_loading.len() >= 4 {
            return;
        }
        self.tree_loading.insert(path.clone());
        let request = volumes::TreeRequest {
            path: path.clone(),
            hidden: self.hidden,
            protected: self.settings.protected,
        };
        let task = cx.background_executor().spawn(async move {
            volumes::bounded::<Vec<volumes::TreeEntry>>(
                "--tree-metadata",
                &serde_json::to_string(&request).unwrap(),
                Duration::from_millis(1800),
            )
            .map_err(|e| e.to_string())
        });
        cx.spawn(async move |view, cx| {
            let children = task.await;
            let _ = view.update(cx, |v, cx| {
                v.tree_loading.remove(&path);
                let children = children.map(|entries| {
                    entries
                        .into_iter()
                        .map(|e| {
                            if e.hidden {
                                v.tree_hidden.insert(e.path.clone());
                            } else {
                                v.tree_hidden.remove(&e.path);
                            }
                            (e.name, e.path)
                        })
                        .collect()
                });
                let anchor = v.navigation_anchor();
                v.tree_children.insert(path.clone(), children);
                if v.nav_expand_pending
                    .as_ref()
                    .is_some_and(|(pending, _)| pending == &path)
                {
                    if let Some((path, depth)) = v.nav_expand_pending.take() {
                        v.expand_navigation_children(&path, depth, cx);
                    }
                }
                v.restore_navigation_anchor(anchor);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn expand_to_current(&mut self, cx: &mut Context<Self>) {
        if special(&self.dir()) {
            return;
        }
        self.computer_open = true;
        let ancestors: Vec<_> = self
            .dir()
            .ancestors()
            .skip(1)
            .map(|p| p.to_path_buf())
            .collect();
        for path in ancestors.into_iter().rev() {
            self.tree_open.insert(path.clone());
            if !self.tree_children.contains_key(&path) {
                self.load_tree(path, cx);
            }
        }
    }
    fn toggle_tree(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if volumes::is_computer(&path) {
            self.computer_open = !self.computer_open;
        } else if path == PathBuf::from("::QuickAccess") {
            let marker = PathBuf::from("::QuickAccessCollapsed");
            if !self.tree_open.remove(&marker) {
                self.tree_open.insert(marker);
            }
        } else if !self.tree_open.remove(&path) {
            self.tree_open.insert(path.clone());
            if !special(&path) {
                self.load_tree(path, cx);
            }
        }
        cx.notify();
    }
    fn append_tree(
        &self,
        path: &std::path::Path,
        label: String,
        depth: usize,
        icon: &str,
        out: &mut Vec<NavRow>,
    ) {
        out.push(NavRow {
            path: path.to_path_buf(),
            label,
            depth,
            expandable: true,
            pinned: false,
            icon: icon.into(),
        });
        if depth >= 32 || !self.tree_open.contains(path) {
            return;
        }
        if let Some(children) = self.tree_children.get(path) {
            match children {
                Ok(children) => {
                    for (name, path) in children {
                        self.append_tree(path, name.clone(), depth + 1, "folder", out);
                        if out.len() > 20000 {
                            break;
                        }
                    }
                }
                Err(_) => out.push(NavRow {
                    label: "无法读取或超时（折叠后可重试）".into(),
                    path: PathBuf::new(),
                    depth: depth + 1,
                    expandable: false,
                    pinned: false,
                    icon: "".into(),
                }),
            }
        } else if self.tree_loading.contains(path) {
            out.push(NavRow {
                label: "正在加载…".into(),
                path: PathBuf::new(),
                depth: depth + 1,
                expandable: false,
                pinned: false,
                icon: "".into(),
            });
        }
    }
    fn nav_rows(&self) -> Vec<NavRow> {
        let mut rows = vec![];
        let mut add = |label: &str, path: PathBuf, icon: &str| {
            rows.push(NavRow {
                label: label.into(),
                path,
                depth: 0,
                expandable: false,
                pinned: false,
                icon: icon.into(),
            })
        };
        add(
            "主文件夹",
            shell::known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile)
                .unwrap_or_else(|| PathBuf::from("C:\\")),
            "home",
        );
        add(
            "图库",
            shell::known_folder(&windows::Win32::UI::Shell::FOLDERID_Pictures)
                .unwrap_or_else(|| PathBuf::from("C:\\")),
            "pictures",
        );
        add(
            "回收站",
            PathBuf::from("shell:RecycleBinFolder"),
            "recycle-bin",
        );
        add("", PathBuf::new(), "");
        rows.push(NavRow {
            label: "快速访问".into(),
            path: PathBuf::from("::QuickAccess"),
            depth: 0,
            expandable: true,
            pinned: false,
            icon: "quick-access".into(),
        });
        if !self
            .tree_open
            .contains(&PathBuf::from("::QuickAccessCollapsed"))
        {
            for (name, path) in self.quick_access_items() {
                let known_name = self
                    .places
                    .iter()
                    .find(|(_, p)| p == &path)
                    .map(|(label, _)| label.as_str())
                    .unwrap_or(name.as_str());
                let icon = match known_name {
                    "桌面" => "desktop",
                    "下载" => "downloads",
                    "文档" => "documents",
                    "图片" => "pictures",
                    "音乐" => "music",
                    "视频" => "videos",
                    _ => "folder",
                };
                rows.push(NavRow {
                    label: name,
                    path: path.clone(),
                    depth: 1,
                    expandable: false,
                    pinned: self.settings.favorites.contains(&path)
                        || self
                            .system_quick_access
                            .iter()
                            .any(|item| item.path == path && item.pinned == Some(true)),
                    icon: icon.into(),
                });
            }
        }
        rows.push(NavRow {
            label: String::new(),
            path: PathBuf::new(),
            depth: 0,
            expandable: false,
            pinned: false,
            icon: "".into(),
        });
        if self.settings.show_all_folders {
            if let Some(path) = shell::known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop) {
                self.append_tree(&path, "桌面".into(), 0, "desktop", &mut rows);
            }
        }
        if self.settings.show_computer {
            rows.push(NavRow {
                label: self.computer_name.clone(),
                path: volumes::computer(),
                depth: 0,
                expandable: true,
                pinned: false,
                icon: "computer".into(),
            });
            if self.computer_open {
                for volume in &self.volumes {
                    self.append_tree(&volume.path, volume.label.clone(), 1, "drive", &mut rows);
                }
            }
        }
        if self.settings.show_libraries {
            rows.push(NavRow {
                label: "库".into(),
                path: PathBuf::from("::Libraries"),
                depth: 0,
                expandable: true,
                pinned: false,
                icon: "documents".into(),
            });
            if self.tree_open.contains(&PathBuf::from("::Libraries")) {
                for (name, path) in &self.places {
                    if matches!(name.as_str(), "文档" | "图片" | "音乐" | "视频") {
                        self.append_tree(path, name.clone(), 1, "folder", &mut rows);
                    }
                }
            }
        }
        if self.settings.show_network {
            rows.push(NavRow {
                label: "网络".into(),
                path: PathBuf::from("::Network"),
                depth: 0,
                expandable: true,
                pinned: false,
                icon: "network".into(),
            });
            if self.tree_open.contains(&PathBuf::from("::Network")) {
                for volume in self.volumes.iter().filter(|v| v.kind == 4) {
                    self.append_tree(&volume.path, volume.label.clone(), 1, "drive", &mut rows);
                }
            }
        }
        rows
    }
    pub(super) fn render_sidebar(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.nav_rows();
        let t = self.theme;
        let row_height = if self.settings.compact { 26. } else { 32. };
        div()
            .key_context("NavigationTree")
            .track_focus(&self.nav_focus)
            .on_action(cx.listener(|v, action: &NavigateTree, window, cx| {
                cx.stop_propagation();
                v.navigate_tree(action.command, window, cx);
            }))
            .w(ui(width))
            .relative()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(t.border))
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, e: &MouseDownEvent, _, cx| {
                    v.popup = Some(("nav-menu".into(), e.position));
                    v.submenu = None;
                    cx.notify();
                }),
            )
            .child(
                uniform_list(
                    "navigation-tree",
                    rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        range
                            .map(|index| {
                                let row = &rows[index];
                                if row.path.as_os_str().is_empty() {
                                    return div()
                                        .w(ui(width))
                                        .h(ui(row_height))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .text_color(rgb(t.muted))
                                        .child(if row.label.is_empty() {
                                            div()
                                                .h(ui(1.))
                                                .w_full()
                                                .bg(rgb(t.muted))
                                                .opacity(0.35)
                                                .into_any_element()
                                        } else {
                                            div().child(row.label.clone()).into_any_element()
                                        })
                                        .into_any_element();
                                }
                                let path = row.path.clone();
                                let arrow_path = path.clone();
                                let right_path = path.clone();
                                let drop_path = path.clone();
                                let external_drop_path = path.clone();
                                let accepts_drop = !special(&path)
                                    && path != PathBuf::from("shell:RecycleBinFolder")
                                    && !this.settings.file_favorites.contains(&path);
                                let expandable = row.expandable;
                                let pinned = row.pinned;
                                let expanded = this.tree_is_open(&path);
                                let keyboard = this.nav_focus.is_focused(window)
                                    && this.nav_keyboard_index == index;
                                let active = if this.nav_focus.is_focused(window) {
                                    keyboard
                                } else {
                                    this.dir() == path
                                };
                                let target =
                                    this.file_drag.as_ref().and_then(|d| d.target.as_ref())
                                        == Some(&path);
                                let bounds_path = path.clone();
                                let drop_bounds = this.file_drop_bounds.clone();
                                div()
                                    .id(("nav", index))
                                    .w(ui((width - 20.).max(100.)))
                                    .h(ui(row_height))
                                    .pl(ui(3. + row.depth as f32 * 16.))
                                    .pr_2()
                                    .mx_1()
                                    .rounded(ui(3.))
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when(active || target, |d| d.bg(rgb(t.selection)))
                                    .when(this.tree_hidden.contains(&row.path), |d| d.opacity(0.5))
                                    .hover(move |s| {
                                        s.bg(rgb(if target { t.selection } else { t.hover }))
                                    })
                                    .cursor_pointer()
                                    .when(accepts_drop, |d| {
                                        d.child(
                                            canvas(
                                                move |bounds, _, _| {
                                                    drop_bounds.borrow_mut().push((
                                                        bounds_path.clone(),
                                                        bounds,
                                                        true,
                                                    ));
                                                },
                                                |_, _, _, _| {},
                                            )
                                            .absolute()
                                            .inset_0(),
                                        )
                                        .on_drop(cx.listener(move |v, drag: &DragFiles, w, cx| {
                                            cx.stop_propagation();
                                            v.confirm_drop(
                                                drag.paths.clone(),
                                                drop_path.clone(),
                                                w.modifiers().control,
                                                cx,
                                            );
                                        }))
                                        .on_drop(
                                            cx.listener(move |v, drag: &ExternalPaths, w, cx| {
                                                cx.stop_propagation();
                                                v.confirm_drop(
                                                    drag.paths().to_vec(),
                                                    external_drop_path.clone(),
                                                    w.modifiers().control,
                                                    cx,
                                                );
                                            }),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id(("tree-toggle", index))
                                            .w(ui(16.))
                                            .h_full()
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(row.expandable, |d| {
                                                d.child(visual::icon(
                                                    if expanded {
                                                        "chevron-down"
                                                    } else {
                                                        "chevron-right"
                                                    },
                                                    t.muted,
                                                ))
                                            })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |v, _, window, cx| {
                                                    cx.stop_propagation();
                                                    v.nav_keyboard_index = index;
                                                    v.keyboard_focus_region = 3;
                                                    window.focus(&v.nav_focus);
                                                    if expandable {
                                                        v.toggle_tree(arrow_path.clone(), cx);
                                                    }
                                                }),
                                            ),
                                    )
                                    .child(if row.icon == "drive" {
                                        visual::file_icon(&row.path, true, t.muted)
                                    } else {
                                        visual::icon(
                                            if row.icon == "computer" {
                                                "computer-colored"
                                            } else {
                                                &row.icon
                                            },
                                            t.text,
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(row.label.clone()),
                                    )
                                    .when(pinned, |d| d.child(visual::icon("pin", t.muted)))
                                    .when(
                                        !pinned
                                            && this
                                                .system_quick_access
                                                .iter()
                                                .any(|item| item.path == row.path),
                                        |d| {
                                            d.child(
                                                div()
                                                    .opacity(0.45)
                                                    .child(visual::icon("pin", t.muted)),
                                            )
                                        },
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |v, _, window, cx| {
                                            cx.stop_propagation();
                                            v.nav_keyboard_index = index;
                                            v.keyboard_focus_region = 3;
                                            window.focus(&v.nav_focus);
                                            if v.settings.file_favorites.contains(&path) {
                                                v.run_shell(
                                                    shell::Request::Open { path: path.clone() },
                                                    cx,
                                                );
                                            } else {
                                                v.navigate_with_modifiers(
                                                    path.clone(),
                                                    window.modifiers(),
                                                    cx,
                                                );
                                            }
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |v, e: &MouseDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            v.nav_keyboard_index = index;
                                            v.keyboard_focus_region = 3;
                                            window.focus(&v.nav_focus);
                                            v.navigation_context_menu(
                                                right_path.clone(),
                                                e.position,
                                                cx,
                                            );
                                        }),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.nav_scroll.clone())
                .flex_1()
                .min_h_0(),
            )
            .child(self.auxiliary_scrollbar(
                self.nav_scroll.0.borrow().base_handle.clone(),
                "navigation-scrollbar",
                cx,
            ))
            .into_any_element()
    }
    fn volume_item(
        &self,
        index: usize,
        volume: &Volume,
        width: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let mode = self.view;
        let grid = matches!(
            mode,
            ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium
        );
        let small = matches!(mode, ViewMode::Small | ViewMode::List);
        let details = mode == ViewMode::Details;
        let content = mode == ViewMode::Content;
        let tiles = mode == ViewMode::Tiles;
        let size = if grid {
            mode.icon_size()
        } else if small {
            16.
        } else if details {
            20.
        } else if content {
            64.
        } else if tiles {
            rust_explorer::layout_metrics::DRIVE_TILE_ICON
        } else {
            48.
        };
        let path = volume.path.clone();
        let menu_path = path.clone();
        let drop_path = path.clone();
        let external_drop_path = path.clone();
        let bounds_path = path.clone();
        let drop_bounds = self.file_drop_bounds.clone();
        let selected = self.selected.contains(&index);
        let target = self.file_drag.as_ref().and_then(|d| d.target.as_ref()) == Some(&path);
        let used = match (volume.total, volume.free) {
            (Some(total), Some(free)) if total > 0 => {
                ((total - free.min(total)) as f32 / total as f32).clamp(0., 1.)
            }
            _ => 0.,
        };
        let capacity = volume.total.is_some_and(|n| n > 0)
            && volume.free.is_some_and(|n| n > 0)
            && !volume.read_only;
        let space = match (volume.total, volume.free) {
            (Some(total), Some(free)) => format!(
                "{} 可用，共 {}",
                fs::size_label(free),
                fs::size_label(total)
            ),
            _ => volume
                .remote
                .clone()
                .unwrap_or_else(|| "空间信息不可用".into()),
        };
        let label = |value: String| {
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(value)
        };
        let mut row = div()
            .id(("volume", index))
            .w(ui(width))
            .flex_shrink_0()
            .relative()
            .rounded(ui(3.))
            .flex()
            .items_center()
            .when(!details && !tiles, |d| d.p_2().gap_2())
            .when(tiles, |d| {
                d.h(ui(rust_explorer::layout_metrics::DRIVE_TILE_HEIGHT))
                    .px_1()
                    .py_1()
                    .gap(ui(rust_explorer::layout_metrics::DRIVE_TILE_GAP))
                    .overflow_hidden()
            })
            .when(details, |d| {
                d.h(ui(if self.settings.compact { 26. } else { 34. }))
            })
            .when(grid, |d| d.flex_col().justify_start().min_h(ui(size + 52.)))
            .when(small, |d| {
                d.h(ui(if self.settings.compact { 28. } else { 34. }))
            })
            .when(!grid && !small && !details && !tiles, |d| {
                d.min_h(ui(if content {
                    if self.settings.compact { 80. } else { 92. }
                } else if self.settings.compact {
                    64.
                } else {
                    76.
                }))
            })
            .bg(rgb(if selected || target {
                t.selection
            } else {
                t.background
            }))
            .hover(move |s| {
                s.bg(rgb(if selected || target {
                    if t.dark { 0x465569 } else { 0xb8d7f3 }
                } else {
                    t.hover
                }))
            })
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| {
                        drop_bounds
                            .borrow_mut()
                            .push((bounds_path.clone(), bounds, true));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .on_drop(cx.listener(move |v, drag: &DragFiles, w, cx| {
                cx.stop_propagation();
                v.confirm_drop(
                    drag.paths.clone(),
                    drop_path.clone(),
                    w.modifiers().control,
                    cx,
                );
            }))
            .on_drop(cx.listener(move |v, drag: &ExternalPaths, w, cx| {
                cx.stop_propagation();
                v.confirm_drop(
                    drag.paths().to_vec(),
                    external_drop_path.clone(),
                    w.modifiers().control,
                    cx,
                );
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    v.select(index, e.modifiers.control, e.modifiers.shift, w, cx);
                    if e.click_count == 2 || v.settings.single_click {
                        v.navigate_with_modifiers(path.clone(), e.modifiers, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    if !v.selected.contains(&index) {
                        v.select(index, false, false, w, cx);
                    }
                    v.context_target = Some(menu_path.clone());
                    v.menu(cx);
                    if let Some((_, position)) = &mut v.popup {
                        *position = e.position;
                    }
                }),
            );
        if details {
            let kind = volume_type(volume);
            for (key, cell) in &self.settings.volume_columns {
                let value = match key.as_str() {
                    "name" => volume.label.clone(),
                    "kind" => kind.into(),
                    "total" => volume.total.map(fs::size_label).unwrap_or_default(),
                    "free" => volume.free.map(fs::size_label).unwrap_or_default(),
                    _ => String::new(),
                };
                let mut field = div()
                    .w(ui(*cell))
                    .h_full()
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .min_w_0();
                if key == "name" {
                    field = field.gap_2().child(visual::file_icon_sized(
                        &volume.path,
                        true,
                        t.text,
                        size,
                    ));
                }
                if key == "total" || key == "free" {
                    field = field.justify_end();
                }
                row = row.child(field.child(label(value).min_w_0()));
            }
        } else {
            row = row.child(visual::file_icon_sized(&volume.path, true, t.text, size));
            let mut text = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .when(tiles, |d| d.gap(ui(1.)).line_height(ui(16.)))
                .when(!grid, |d| d.flex_1())
                .when(grid, |d| d.w_full().text_center())
                .child(if grid {
                    icon_label(
                        &volume.label,
                        (width - 16.).max(24.),
                        match mode {
                            ViewMode::ExtraLarge => 15.,
                            ViewMode::Large => 14.,
                            _ => 13.,
                        },
                        self.zoom,
                        window,
                    )
                } else {
                    label(volume.label.clone())
                        .text_size(ui(13.))
                        .into_any_element()
                });
            if !small && !grid {
                if capacity {
                    text = text.child(
                        div()
                            .h(ui(if content {
                                20.
                            } else if tiles {
                                rust_explorer::layout_metrics::DRIVE_TILE_BAR_HEIGHT
                            } else {
                                16.
                            }))
                            .w_full()
                            .when(tiles, |d| {
                                d.w(ui(rust_explorer::layout_metrics::DRIVE_TILE_BAR_WIDTH))
                            })
                            .border_1()
                            .border_color(rgb(0xbcbcbc))
                            .bg(rgb(0xe6e6e6))
                            .child(div().h_full().w(relative(used)).bg(rgb(if used > 0.9 {
                                0xc42b1c
                            } else {
                                0x0078d4
                            }))),
                    );
                }
                text = text.child(
                    label(space)
                        .text_size(ui(12.))
                        .text_color(rgb(if tiles && t.dark { t.text } else { t.muted }))
                        .when(tiles, |d| d.line_height(ui(14.))),
                );
                if content || volume.kind == 5 {
                    text = text.child(
                        label(format!(
                            "{}{}",
                            volume_type(volume),
                            if volume.filesystem.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", volume.filesystem)
                            }
                        ))
                        .text_size(ui(12.))
                        .when(tiles, |d| d.line_height(ui(14.)))
                        .text_color(rgb(t.muted)),
                    );
                }
            }
            row = row.child(text);
        }
        row.into_any_element()
    }
    fn render_volume_views(
        &self,
        mut body: Stateful<Div>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let list = self.view == ViewMode::List;
        let details = self.view == ViewMode::Details;
        let grid = matches!(
            self.view,
            ViewMode::ExtraLarge | ViewMode::Large | ViewMode::Medium
        );
        let available = (self.layout_width - 20.).max(120.);
        let table_width: f32 = self
            .settings
            .volume_columns
            .iter()
            .map(|(_, width)| *width)
            .sum();
        let grouped = !self.group.is_empty();
        let mut groups: Vec<(String, Vec<(usize, &Volume)>)> = vec![];
        let drive_entries: Vec<_> = self.entries.iter().enumerate().collect();
        for (index, entry) in drive_entries {
            let Some(volume) = self.volumes.iter().find(|v| v.path == entry.path) else {
                continue;
            };
            let title = match self.group.as_str() {
                "kind" => {
                    if volume.kind == 4 {
                        "网络位置".into()
                    } else {
                        "设备和驱动器".into()
                    }
                }
                "name" => volume
                    .label
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default(),
                "size" => {
                    if volume.total.unwrap_or(0) >= 1024 * 1024 * 1024 * 1024 {
                        "1 TB 及以上".into()
                    } else {
                        "小于 1 TB".into()
                    }
                }
                _ => String::new(),
            };
            if let Some((_, items)) = groups.iter_mut().find(|(name, _)| name == &title) {
                items.push((index, volume));
            } else {
                groups.push((title, vec![(index, volume)]));
            }
        }
        if list {
            body = body.flex_row().items_start().gap_4();
        }
        if details {
            let mut header = div()
                .w(ui(table_width))
                .h(ui(30.))
                .flex()
                .flex_shrink_0()
                .border_b_1()
                .border_color(rgb(t.border));
            for (index, (key, width)) in self.settings.volume_columns.iter().enumerate() {
                let label = match key.as_str() {
                    "kind" => "\u{7c7b}\u{578b}",
                    "total" => "\u{603b}\u{5927}\u{5c0f}",
                    "free" => "\u{53ef}\u{7528}\u{7a7a}\u{95f4}",
                    _ => "\u{540d}\u{79f0}",
                };
                let sort_key = key.clone();
                let width = *width;
                let sorted = self.settings.volume_sort == *key;
                header = header.child(
                    div()
                        .id(("volume-column", index))
                        .relative()
                        .w(ui(width))
                        .h_full()
                        .flex_shrink_0()
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_1()
                        .hover(move |s| s.bg(rgb(t.hover)))
                        .child(label)
                        .when(sorted, |d| {
                            d.child(visual::icon(
                                if self.settings.volume_descending {
                                    "chevron-down"
                                } else {
                                    "chevron-up"
                                },
                                t.muted,
                            ))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.settings.volume_descending = v.settings.volume_sort == sort_key
                                    && !v.settings.volume_descending;
                                v.settings.volume_sort = sort_key.clone();
                                v.sort = match sort_key.as_str() {
                                    "kind" => Sort::Kind,
                                    "total" | "free" => Sort::Size,
                                    _ => Sort::Name,
                                };
                                v.descending = v.settings.volume_descending;
                                v.sync_computer_entries();
                                v.save_settings(cx);
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .id(("resize-volume-column", index))
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
                                        v.volume_columns_resize =
                                            Some((index, e.position.x.into(), width));
                                    }),
                                )
                                .on_click(|_, _, cx| cx.stop_propagation()),
                        ),
                );
            }
            body = body.child(header);
        }
        for (group_index, (title, items)) in groups.into_iter().enumerate() {
            let collapsed = !list && self.device_groups.contains(&title);
            let group_width = if list {
                280.
            } else if details {
                table_width
            } else {
                available
            };
            let mut section = div()
                .w(ui(group_width))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap_1();
            if grouped && !title.is_empty() {
                let key = title.clone();
                let indices: Vec<_> = items.iter().map(|(index, _)| *index).collect();
                section = section.child(
                    div()
                        .id(("device-group", group_index))
                        .h(ui(30.))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_color(rgb(t.text))
                        .hover(move |s| s.bg(rgb(t.hover)))
                        .child(visual::icon(
                            if collapsed {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            },
                            t.text,
                        ))
                        .child(format!("{title} ({})", items.len()))
                        .child(div().flex_1().h(ui(1.)).bg(rgb(t.border)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                                cx.stop_propagation();
                                if e.click_count == 2 && !list {
                                    if !v.device_groups.remove(&key) {
                                        v.device_groups.insert(key.clone());
                                    }
                                } else if e.click_count == 1 {
                                    if !e.modifiers.control {
                                        v.selected.clear();
                                    }
                                    v.selected.extend(indices.iter().copied());
                                    v.focus_item = indices
                                        .first()
                                        .and_then(|i| v.entries.get(*i))
                                        .map(|e| e.path.clone());
                                    v.clear_preview();
                                    w.focus(&v.focus);
                                }
                                cx.notify();
                            }),
                        ),
                );
            }
            if !collapsed {
                let cell = if grid {
                    self.view.icon_size() + 64.
                } else if self.view == ViewMode::Small {
                    260.
                } else if self.view == ViewMode::Tiles {
                    rust_explorer::layout_metrics::DRIVE_TILE_WIDTH
                } else {
                    group_width
                };
                let columns = if grid || matches!(self.view, ViewMode::Small | ViewMode::Tiles) {
                    (available / cell).floor().max(1.)
                } else {
                    1.
                };
                let width = if self.view == ViewMode::Content || details || list {
                    group_width
                } else if self.view == ViewMode::Tiles {
                    rust_explorer::layout_metrics::DRIVE_TILE_WIDTH
                } else {
                    (available / columns).floor()
                };
                let mut content = div()
                    .flex()
                    .items_start()
                    .when(!list && !details && self.view != ViewMode::Content, |d| {
                        d.flex_wrap()
                    })
                    .when(list || details || self.view == ViewMode::Content, |d| {
                        d.flex_col()
                    });
                for (index, volume) in items {
                    content = content.child(self.volume_item(index, volume, width, window, cx));
                }
                section = section.child(content);
            }
            body = body.child(section);
        }
        if self.entries.is_empty() {
            body = body.child(
                div()
                    .p_4()
                    .text_color(rgb(t.muted))
                    .child(if self.volume_busy {
                        "正在更新设备…"
                    } else {
                        "没有可显示的存储设备或映射网络位置"
                    }),
            );
        }
        body.into_any_element()
    }

    pub(super) fn render_computer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let mut body = div()
            .id("computer-home")
            .overflow_y_scroll()
            .overflow_x_scroll()
            .track_scroll(&self.home_scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, e: &MouseDownEvent, w, cx| v.begin_marquee(e, w, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    v.selected.clear();
                    v.clear_preview();
                    v.menu(cx);
                    if let Some((_, p)) = &mut v.popup {
                        *p = e.position;
                    }
                }),
            )
            .flex_1()
            .min_h_0()
            .p_2()
            .flex()
            .flex_col()
            .gap_1();
        if self.view == ViewMode::List {
            let weak = cx.entity().downgrade();
            let scroll = self.home_scroll.clone();
            body = body.child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let weak = weak.clone();
                        let scroll = scroll.clone();
                        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture
                                || !scroll.bounds().contains(&e.position)
                                || e.modifiers.control
                            {
                                return;
                            }
                            let _ = weak.update(cx, |v, cx| {
                                if v.popup.is_some() || v.options.is_some() || v.dialog.is_some() {
                                    return;
                                }
                                let max = f32::from(scroll.max_offset().width);
                                if max <= 1. {
                                    return;
                                }
                                let delta = e.delta.pixel_delta(px(40.));
                                let amount = if f32::from(delta.x).abs() > 0.1 {
                                    f32::from(delta.x)
                                } else {
                                    f32::from(delta.y)
                                };
                                scroll.set_offset(point(
                                    px((f32::from(scroll.offset().x) + amount).clamp(-max, 0.)),
                                    scroll.offset().y,
                                ));
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        }
        if self.dir() == PathBuf::from("::QuickAccess") {
            let mut cards = div().flex().flex_wrap().gap_2();
            for (index, entry) in self.entries.iter().enumerate() {
                let path = entry.path.clone();
                let menu_path = path.clone();
                let bounds_path = path.clone();
                let drop_bounds = self.file_drop_bounds.clone();
                let selected = self.selected.contains(&index);
                cards = cards.child(
                    div()
                        .id(("quick-card", index))
                        .w(ui(280.))
                        .h(ui(if self.settings.compact { 64. } else { 80. }))
                        .relative()
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .rounded(ui(4.))
                        .bg(rgb(if selected { t.selection } else { t.background }))
                        .hover(move |s| {
                            s.bg(rgb(if selected {
                                if t.dark { 0x465569 } else { 0xb8d7f3 }
                            } else {
                                t.hover
                            }))
                        })
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    drop_bounds.borrow_mut().push((
                                        bounds_path.clone(),
                                        bounds,
                                        true,
                                    ))
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .child(visual::file_icon_sized(&path, true, t.text, 48.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(entry.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(ui(12.))
                                        .text_color(rgb(t.muted))
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(path.display().to_string()),
                                ),
                        )
                        .child(visual::icon("pin", t.muted))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                                cx.stop_propagation();
                                v.select(index, e.modifiers.control, e.modifiers.shift, w, cx);
                                if e.click_count == 2 || v.settings.single_click {
                                    v.open_selected(cx);
                                }
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |v, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                v.context_target = Some(menu_path.clone());
                                v.popup = Some(("quick-menu".into(), e.position));
                                v.submenu = None;
                                cx.notify();
                            }),
                        ),
                );
            }
            return body
                .child(
                    div()
                        .py_2()
                        .child(format!("常用文件夹 ({})", self.entries.len())),
                )
                .child(cards)
                .into_any_element();
        }
        if self.dir() == PathBuf::from("::Libraries") {
            for (index, e) in self.entries.iter().enumerate() {
                let path = e.path.clone();
                body = body.child(
                    button(("library", index), e.name.clone(), t)
                        .justify_start()
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.navigate_with_modifiers(path.clone(), window.modifiers(), cx)
                        })),
                );
            }
            return body.into_any_element();
        }
        self.render_volume_views(body, window, cx)
    }
}
