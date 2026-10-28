use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(test)]
mod interaction_defaults_tests {
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
    pub separate_process: bool,
    pub full_path: bool,
    pub preview: bool,
    pub details_pane: bool,
    pub folders_first: bool,
    pub remember_views: bool,
    pub columns: Vec<(String, f32)>,
    pub recycle_columns: Vec<(String, f32)>,
    pub search_columns: Vec<(String, f32)>,
    pub volume_columns: Vec<(String, f32)>,
    pub volume_sort: String,
    pub volume_descending: bool,
    pub folder_views: std::collections::BTreeMap<PathBuf, ViewMode>,
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
            separate_process: false,
            full_path: false,
            preview: true,
            details_pane: false,
            folders_first: true,
            remember_views: true,
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
    pub fn enqueue_save(&self) {
        static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<Settings>> =
            std::sync::OnceLock::new();
        let sender = WRITER.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Settings>();
            std::thread::spawn(move || {
                while let Ok(mut settings) = rx.recv() {
                    for newer in rx.try_iter() {
                        settings = newer;
                    }
                    settings.save();
                }
            });
            tx
        });
        let _ = sender.send(self.clone());
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
