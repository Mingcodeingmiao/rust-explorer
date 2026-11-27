use crate::fs::Sort;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};

#[cfg(test)]
mod interaction_defaults_tests {
    use super::{FolderView, Settings, ViewMode};
    use crate::fs::Sort;

    #[test]
    fn stale_window_edits_preserve_other_changes_and_favorites() {
        use std::path::PathBuf;
        let base = Settings::default();
        let mut first = base.clone();
        first.theme = "dark".into();
        first.favorites.push(PathBuf::from(r"C:\first"));
        first
            .folder_views
            .insert(PathBuf::from(r"C:\first"), FolderView::default());
        let mut second = base.clone();
        second.preview = !base.preview;
        second.favorites.push(PathBuf::from(r"C:\second"));
        second
            .folder_views
            .insert(PathBuf::from(r"C:\second"), FolderView::default());

        let mut saved = first.clone();
        saved.merge_changes(&base, &second);
        assert_eq!(saved.theme, "dark");
        assert_eq!(saved.preview, second.preview);
        assert_eq!(
            saved.favorites,
            [PathBuf::from(r"C:\first"), PathBuf::from(r"C:\second")]
        );
        assert_eq!(saved.folder_views.len(), 2);

        let mut removed = first.clone();
        removed.favorites.clear();
        removed.folder_views.clear();
        saved.merge_changes(&first, &removed);
        assert_eq!(saved.favorites, [PathBuf::from(r"C:\second")]);
        assert!(
            saved
                .folder_views
                .contains_key(&PathBuf::from(r"C:\second"))
        );
    }
    use std::path::Path;

    #[test]
    fn folder_views_round_trip_with_legacy_view_modes() {
        let mut settings: Settings =
            serde_json::from_str(r#"{"folder_views":{"C:\\Pictures":"Large"}}"#).unwrap();
        assert_eq!(
            settings.folder_views[Path::new(r"C:\Pictures")],
            FolderView {
                mode: Some(ViewMode::Large),
                ..FolderView::default()
            }
        );
        settings.folder_views.insert(
            r"C:\Documents".into(),
            FolderView {
                mode: Some(ViewMode::List),
                sort: Some((Sort::Modified, true)),
                group: Some("kind".into()),
                group_descending: true,
            },
        );
        settings.folder_views.insert(
            "::ThisPC".into(),
            FolderView {
                group: Some(String::new()),
                ..FolderView::default()
            },
        );
        let decoded: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded.folder_views, settings.folder_views);
    }

    #[test]
    fn language_defaults_to_english_and_round_trips() {
        let settings: super::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.language, "en-US");
        for tag in ["en-US", "zh-CN", "ja-JP", "ko-KR"] {
            let settings = super::Settings {
                language: tag.into(),
                ..Default::default()
            };
            let decoded: super::Settings =
                serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
            assert_eq!(decoded.language, tag);
        }
    }
    #[test]
    fn old_settings_do_not_enable_blank_double_click_navigation() {
        let settings: super::Settings = serde_json::from_str("{}").unwrap();
        assert!(!settings.blank_double_click_up);
        assert!(settings.thumbnail_cache_directory.is_none());
        let mut settings = settings;
        settings.blank_double_click_up = true;
        settings.thumbnail_cache_directory = Some("D:\\RE thumbnails".into());
        let decoded: super::Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(decoded.blank_double_click_up);
        assert_eq!(
            settings.thumbnail_cache_directory,
            decoded.thumbnail_cache_directory
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    ExtraLarge,
    Large,
    Medium,
    Small,
    List,
    #[default]
    Details,
    Tiles,
    Content,
}
impl ViewMode {
    pub const ALL: [(Self, &'static str); 8] = [
        (Self::ExtraLarge, "超大图标"),
        (Self::Large, "大图标"),
        (Self::Medium, "中等图标"),
        (Self::Small, "小图标"),
        (Self::List, "列表"),
        (Self::Details, "详细信息"),
        (Self::Tiles, "平铺"),
        (Self::Content, "内容"),
    ];
    pub fn icon_size(self) -> f32 {
        match self {
            // 257 = the previous 154 enlarged by 1.67, closer to Explorer's jumbo icons.
            Self::ExtraLarge => 257.,
            Self::Large => 102.,
            Self::Medium => 64.,
            Self::Tiles | Self::Content => 40.,
            _ => 16.,
        }
    }
}

pub type FolderViews = BTreeMap<PathBuf, FolderView>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredFolderView")]
pub struct FolderView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<ViewMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<(Sort, bool)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub group_descending: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredFolderView {
    Legacy(ViewMode),
    Listing {
        mode: Option<ViewMode>,
        sort: Option<(Sort, bool)>,
        group: Option<String>,
        #[serde(default)]
        group_descending: bool,
    },
}

impl From<StoredFolderView> for FolderView {
    fn from(stored: StoredFolderView) -> Self {
        match stored {
            StoredFolderView::Legacy(mode) => Self {
                mode: Some(mode),
                ..Self::default()
            },
            StoredFolderView::Listing {
                mode,
                sort,
                group,
                group_descending,
            } => Self {
                mode,
                sort,
                group,
                group_descending,
            },
        }
    }
}

enum SettingsWrite {
    Change(Box<(Settings, Settings)>),
    Flush(mpsc::Sender<()>),
}
static SETTINGS_WRITER: OnceLock<mpsc::Sender<SettingsWrite>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: String,
    pub view: ViewMode,
    pub extensions: bool,
    pub hidden: bool,
    pub protected: bool,
    pub checkboxes: bool,
    pub compact: bool,
    pub high_performance: bool,
    pub thumbnails: bool,
    /// Load every thumbnail of a folder as soon as it opens instead of only the
    /// rows that are scrolled into view.
    pub immediate_thumbnails: bool,
    pub blank_double_click_up: bool,
    pub thumbnail_cache_directory: Option<PathBuf>,
    pub wrap_names: bool,
    pub single_click: bool,
    #[serde(alias = "shift_new_tab")]
    pub shift_open_tab: bool,
    pub quick_access_new_tab: bool,
    pub new_tab_this_pc: bool,
    /// Opening a folder in a new tab jumps to a tab that already shows it.
    pub focus_existing_tab: bool,
    pub startup_page: String,
    pub window_placement: Option<WindowPlacement>,
    pub separate_process: bool,
    pub full_path: bool,
    pub preview: bool,
    pub details_pane: bool,
    pub folders_first: bool,
    pub remember_views: bool,
    pub check_updates: bool,
    pub columns: Vec<(String, f32)>,
    pub recycle_columns: Vec<(String, f32)>,
    pub search_columns: Vec<(String, f32)>,
    pub volume_columns: Vec<(String, f32)>,
    pub volume_sort: String,
    pub volume_descending: bool,
    pub folder_views: FolderViews,
    pub favorites: Vec<PathBuf>,
    pub show_computer: bool,
    pub show_network: bool,
    pub show_libraries: bool,
    pub show_all_folders: bool,
    pub expand_current: bool,
    pub quick_access_initialized: bool,
    pub quick_access_system_migrated: bool,
    pub quick_access_hidden: Vec<PathBuf>,
    pub file_favorites: Vec<PathBuf>,
    pub sidebar_width: f32,
    pub preview_width: f32,
    pub theme: String,
    pub full_system_menu: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "en-US".into(),
            view: ViewMode::Details,
            extensions: true,
            hidden: false,
            protected: false,
            checkboxes: false,
            compact: false,
            high_performance: false,
            thumbnails: true,
            immediate_thumbnails: false,
            blank_double_click_up: false,
            thumbnail_cache_directory: None,
            wrap_names: true,
            single_click: false,
            shift_open_tab: false,
            quick_access_new_tab: true,
            new_tab_this_pc: false,
            focus_existing_tab: true,
            startup_page: "home".into(),
            window_placement: None,
            separate_process: false,
            full_path: false,
            preview: true,
            details_pane: false,
            folders_first: true,
            remember_views: true,
            check_updates: true,
            search_columns: vec![
                ("name".into(), 280.),
                ("location".into(), 320.),
                ("modified".into(), 156.),
                ("kind".into(), 112.),
                ("size".into(), 100.),
            ],
            columns: vec![
                ("name".into(), 300.),
                ("modified".into(), 156.),
                ("kind".into(), 112.),
                ("size".into(), 110.),
            ],
            volume_columns: default_volume_columns(),
            recycle_columns: default_recycle_columns(),
            volume_sort: "name".into(),
            volume_descending: false,
            folder_views: Default::default(),
            favorites: vec![],
            show_computer: true,
            show_network: true,
            show_libraries: false,
            show_all_folders: false,
            expand_current: false,
            quick_access_initialized: false,
            quick_access_system_migrated: false,
            quick_access_hidden: vec![],
            file_favorites: vec![],
            sidebar_width: 220.,
            preview_width: 280.,
            theme: "system".into(),
            full_system_menu: false,
        }
    }
}
impl Settings {
    /// Apply only changes made since the window (or dialog) took its snapshot.
    pub fn merge_changes(&mut self, before: &Self, after: &Self) {
        macro_rules! replace_changed {
            ($($field:ident),+ $(,)?) => {$(
                if before.$field != after.$field {
                    self.$field = after.$field.clone();
                }
            )+};
        }
        replace_changed!(
            language,
            view,
            extensions,
            hidden,
            protected,
            checkboxes,
            compact,
            high_performance,
            thumbnails,
            immediate_thumbnails,
            blank_double_click_up,
            thumbnail_cache_directory,
            wrap_names,
            single_click,
            shift_open_tab,
            quick_access_new_tab,
            new_tab_this_pc,
            focus_existing_tab,
            startup_page,
            window_placement,
            separate_process,
            full_path,
            preview,
            details_pane,
            folders_first,
            remember_views,
            check_updates,
            columns,
            recycle_columns,
            search_columns,
            volume_columns,
            volume_sort,
            volume_descending,
            show_computer,
            show_network,
            show_libraries,
            show_all_folders,
            expand_current,
            quick_access_initialized,
            quick_access_system_migrated,
            sidebar_width,
            preview_width,
            theme,
            full_system_menu,
        );
        for path in before.folder_views.keys() {
            if !after.folder_views.contains_key(path) {
                self.folder_views.remove(path);
            }
        }
        for (path, view) in &after.folder_views {
            let previous = before.folder_views.get(path);
            if previous == Some(view) {
                continue;
            }
            let empty = FolderView::default();
            let previous = previous.unwrap_or(&empty);
            let current = self.folder_views.entry(path.clone()).or_default();
            if previous.mode != view.mode {
                current.mode = view.mode;
            }
            if previous.sort != view.sort {
                current.sort = view.sort;
            }
            if previous.group != view.group {
                current.group.clone_from(&view.group);
            }
            if previous.group_descending != view.group_descending {
                current.group_descending = view.group_descending;
            }
        }
        fn merge_paths(current: &mut Vec<PathBuf>, before: &[PathBuf], after: &[PathBuf]) {
            if before == after {
                return;
            }
            current.retain(|path| !before.contains(path) || after.contains(path));
            for path in after {
                if !before.contains(path) && !current.contains(path) {
                    current.push(path.clone());
                }
            }
        }
        merge_paths(&mut self.favorites, &before.favorites, &after.favorites);
        merge_paths(
            &mut self.quick_access_hidden,
            &before.quick_access_hidden,
            &after.quick_access_hidden,
        );
        merge_paths(
            &mut self.file_favorites,
            &before.file_favorites,
            &after.file_favorites,
        );
    }
    /// UI preferences should not re-enumerate a potentially slow network folder.
    pub fn listing_requires_reload(&self, previous: &Self) -> bool {
        self.hidden != previous.hidden
            || self.protected != previous.protected
            || self.folders_first != previous.folders_first
            || self.remember_views != previous.remember_views
            || self.view != previous.view
    }
    fn path() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("RustExplorer/settings.json")
    }
    pub fn exists() -> bool {
        Self::path().is_file()
    }
    pub fn load() -> Self {
        let mut s: Self = std::fs::read(Self::path())
            .ok()
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or_default();
        if !["home", "this-pc", "quick-access"].contains(&s.startup_page.as_str()) {
            s.startup_page = "home".into();
        }
        let allowed = ["name", "modified", "kind", "size", "created"];
        let mut seen = std::collections::HashSet::new();
        s.columns.retain_mut(|(id, w)| {
            *w = w.clamp(64., 1200.);
            allowed.contains(&id.as_str()) && seen.insert(id.clone())
        });
        if !s.columns.iter().any(|(id, _)| id == "name") {
            s.columns.insert(0, ("name".into(), 300.));
        }
        let mut search_seen = std::collections::HashSet::new();
        s.search_columns.retain_mut(|(id, w)| {
            *w = if w.is_finite() {
                w.clamp(64., 1200.)
            } else {
                160.
            };
            ["name", "location", "modified", "kind", "size", "created"].contains(&id.as_str())
                && search_seen.insert(id.clone())
        });
        if !s.search_columns.iter().any(|(id, _)| id == "name") {
            s.search_columns.insert(0, ("name".into(), 280.));
        }
        if !s.search_columns.iter().any(|(id, _)| id == "location") {
            s.search_columns.push(("location".into(), 320.));
        }
        normalize_volume_columns(&mut s.volume_columns);
        normalize_recycle_columns(&mut s.recycle_columns);
        if !["name", "kind", "total", "free"].contains(&s.volume_sort.as_str()) {
            s.volume_sort = "name".into();
        }
        s
    }
    pub fn save(&self) {
        let path = Self::path();
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }
    pub fn enqueue_changes(&self, before: Self) {
        let sender = SETTINGS_WRITER.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<SettingsWrite>();
            std::thread::spawn(move || {
                let mut pending = Vec::new();
                while let Ok(first) = rx.recv() {
                    for request in std::iter::once(first).chain(rx.try_iter()) {
                        match request {
                            SettingsWrite::Change(change) => pending.push(change),
                            SettingsWrite::Flush(done) => {
                                Self::write_changes(&mut pending);
                                let _ = done.send(());
                            }
                        }
                    }
                    Self::write_changes(&mut pending);
                }
            });
            tx
        });
        let _ = sender.send(SettingsWrite::Change(Box::new((before, self.clone()))));
    }

    fn write_changes(pending: &mut Vec<Box<(Self, Self)>>) {
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::{Duration, Instant};
        if pending.is_empty() {
            return;
        }
        let lock_path = Self::path().with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let _lock = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&lock_path)
            {
                Ok(lock) => break lock,
                Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(error) => {
                    eprintln!("Could not lock Rust Explorer settings: {error}");
                    return;
                }
            }
        };
        let mut settings = Self::load();
        for change in pending.drain(..) {
            settings.merge_changes(&change.0, &change.1);
        }
        settings.save();
    }

    pub fn flush_pending_saves() {
        if let Some(writer) = SETTINGS_WRITER.get() {
            let (done, flushed) = mpsc::channel();
            if writer.send(SettingsWrite::Flush(done)).is_ok() {
                let _ = flushed.recv();
            }
        }
    }
}

/// Fixed logical button widths include their spacing; hide in Explorer's priority order.
pub fn toolbar_overflow(width: f32, statistics: bool) -> Vec<&'static str> {
    let mut needed = 1043. + if statistics { 112. } else { 0. };
    let mut hidden = Vec::new();
    for (id, cost) in [
        ("view-menu", 104.),
        ("sort-menu", 104.),
        ("delete", 48.),
        ("share", 48.),
        ("rename", 48.),
        ("paste", 48.),
        ("copy", 48.),
        ("cut", 48.),
        ("usage", 112.),
        ("global", 128.),
        ("new-folder", 94.),
    ] {
        if needed <= width {
            break;
        }
        if id == "usage" && !statistics {
            continue;
        }
        hidden.push(id);
        needed -= cost;
    }
    hidden
}

#[cfg(test)]
mod toolbar_tests {
    #[test]
    fn overflow_is_ordered_and_reversible_by_available_width() {
        assert!(super::toolbar_overflow(1280., true).is_empty());
        let wide = super::toolbar_overflow(1000., true);
        assert_eq!(wide, ["view-menu", "sort-menu"]);
        let narrow = super::toolbar_overflow(640., true);
        assert!(narrow.starts_with(&wide));
        assert_eq!(
            &narrow[..5],
            ["view-menu", "sort-menu", "delete", "share", "rename"]
        );
    }
}

pub fn default_recycle_columns() -> Vec<(String, f32)> {
    [
        ("name", 260.),
        ("original", 270.),
        ("deleted", 170.),
        ("size", 100.),
        ("kind", 150.),
        ("modified", 170.),
    ]
    .into_iter()
    .map(|(id, width)| (id.into(), width))
    .collect()
}
fn default_volume_columns() -> Vec<(String, f32)> {
    [
        ("name", 280.),
        ("kind", 160.),
        ("total", 130.),
        ("free", 130.),
    ]
    .into_iter()
    .map(|(key, width)| (key.into(), width))
    .collect()
}
fn normalize_recycle_columns(columns: &mut Vec<(String, f32)>) {
    let defaults = default_recycle_columns();
    let mut seen = std::collections::HashSet::new();
    columns.retain_mut(|(id, width)| {
        if !defaults.iter().any(|(key, _)| key == id) || !seen.insert(id.clone()) {
            return false;
        }
        let fallback = defaults.iter().find(|(key, _)| key == id).unwrap().1;
        *width = if width.is_finite() {
            width.clamp(64., 1200.)
        } else {
            fallback
        };
        true
    });
    if !columns.iter().any(|(id, _)| id == "name") {
        columns.insert(0, ("name".into(), 260.));
    }
}
fn normalize_volume_columns(columns: &mut Vec<(String, f32)>) {
    let previous = std::mem::take(columns);
    *columns = default_volume_columns()
        .into_iter()
        .map(|(key, default)| {
            let width = previous
                .iter()
                .find(|(id, _)| *id == key)
                .map(|(_, width)| *width)
                .filter(|width| width.is_finite())
                .unwrap_or(default)
                .clamp(64., 1200.);
            (key, width)
        })
        .collect();
}
#[cfg(test)]
mod volume_column_tests {
    #[test]
    fn recycle_columns_keep_their_own_order_widths_and_visibility() {
        let mut settings: super::Settings = serde_json::from_str("{}").unwrap();
        let ordinary = settings.columns.clone();
        settings.recycle_columns = vec![
            ("deleted".into(), 310.),
            ("name".into(), f32::NAN),
            ("deleted".into(), 80.),
            ("unknown".into(), 100.),
        ];
        super::normalize_recycle_columns(&mut settings.recycle_columns);
        assert_eq!(
            settings.recycle_columns,
            vec![("deleted".into(), 310.), ("name".into(), 260.)]
        );
        let saved = serde_json::to_string(&settings).unwrap();
        let loaded: super::Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.recycle_columns, settings.recycle_columns);
        assert_eq!(loaded.columns, ordinary);
    }
    #[test]
    fn reuse_existing_tab_is_on_by_default_and_persisted() {
        // Older settings files predate the option: the default must turn it on.
        let missing: super::Settings = serde_json::from_str("{}").unwrap();
        assert!(missing.focus_existing_tab);
        let saved = serde_json::to_string(&missing).unwrap();
        let mut changed = missing.clone();
        changed.focus_existing_tab = false;
        let reloaded: super::Settings = serde_json::from_str(&saved).unwrap();
        assert!(reloaded.focus_existing_tab);
        let reloaded: super::Settings =
            serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert!(!reloaded.focus_existing_tab);
    }
    #[test]
    fn contributor_and_local_shift_settings_share_one_state() {
        let contributor: super::Settings =
            serde_json::from_str(r#"{"shift_new_tab":true}"#).unwrap();
        let local: super::Settings = serde_json::from_str(r#"{"shift_open_tab":true}"#).unwrap();
        assert!(contributor.shift_open_tab && local.shift_open_tab);
        let saved = serde_json::to_value(contributor).unwrap();
        assert!(saved.get("shift_new_tab").is_none());
        assert_eq!(saved["shift_open_tab"], true);
    }
    #[test]
    fn repairs_invalid_widths_and_preserves_independent_drive_columns() {
        let mut columns = vec![
            ("free".into(), 300.),
            ("name".into(), f32::NAN),
            ("kind".into(), 2.),
            ("total".into(), 5000.),
            ("unknown".into(), 90.),
        ];
        super::normalize_volume_columns(&mut columns);
        assert_eq!(
            columns,
            vec![
                ("name".into(), 280.),
                ("kind".into(), 64.),
                ("total".into(), 1200.),
                ("free".into(), 300.)
            ]
        );
        let settings: super::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.volume_columns, super::default_volume_columns());
        assert_eq!(settings.volume_sort, "name");
    }
}

#[cfg(test)]
mod navigation_preferences_tests {
    use super::Settings;
    #[test]
    fn menu_and_update_preferences_do_not_reload_directory_contents() {
        let original = Settings::default();
        let mut changed = original.clone();
        changed.full_system_menu = true;
        changed.check_updates = false;
        assert!(!changed.listing_requires_reload(&original));
        let loaded: Settings =
            serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(loaded.full_system_menu);
        assert!(!loaded.check_updates);
        changed.hidden = !original.hidden;
        assert!(changed.listing_requires_reload(&original));
        changed.hidden = original.hidden;
        changed.protected = !original.protected;
        assert!(changed.listing_requires_reload(&original));
    }
    #[test]
    fn old_settings_keep_compatible_tab_defaults() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.quick_access_new_tab);
        assert!(!settings.new_tab_this_pc);
        assert_eq!(settings.startup_page, "home");
    }
    #[test]
    fn navigation_choices_survive_save_round_trip() {
        for page in ["home", "this-pc", "quick-access"] {
            let settings = Settings {
                startup_page: page.into(),
                new_tab_this_pc: true,
                quick_access_new_tab: false,
                ..Settings::default()
            };
            let saved = serde_json::to_vec(&settings).unwrap();
            let loaded: Settings = serde_json::from_slice(&saved).unwrap();
            assert_eq!(loaded.startup_page, page);
            assert!(loaded.new_tab_this_pc);
            assert!(!loaded.quick_access_new_tab);
        }
    }
}
