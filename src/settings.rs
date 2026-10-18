use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
            Self::ExtraLarge => 154.,
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
    pub view: ViewMode,
    pub extensions: bool,
    pub hidden: bool,
    pub protected: bool,
    pub checkboxes: bool,
    pub compact: bool,
    pub high_performance: bool,
    pub thumbnails: bool,
    pub wrap_names: bool,
    pub single_click: bool,
    pub shift_open_tab: bool,
    pub separate_process: bool,
    pub full_path: bool,
    pub preview: bool,
    pub details_pane: bool,
    pub folders_first: bool,
    pub remember_views: bool,
    pub columns: Vec<(String, f32)>,
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
            view: ViewMode::Details,
            extensions: true,
            hidden: false,
            protected: false,
            checkboxes: false,
            compact: false,
            high_performance: false,
            thumbnails: true,
            wrap_names: true,
            single_click: false,
            shift_open_tab: false,
            separate_process: false,
            full_path: false,
            preview: true,
            details_pane: false,
            folders_first: true,
            remember_views: true,
            columns: vec![
                ("name".into(), 300.),
                ("modified".into(), 156.),
                ("kind".into(), 112.),
                ("size".into(), 110.),
            ],
            volume_columns: default_volume_columns(),
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
    pub fn load() -> Self {
        let mut s: Self = std::fs::read(Self::path())
            .ok()
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or_default();
        let allowed = ["name", "modified", "kind", "size", "created"];
        let mut seen = std::collections::HashSet::new();
        s.columns.retain_mut(|(id, w)| {
            *w = w.clamp(64., 1200.);
            allowed.contains(&id.as_str()) && seen.insert(id.clone())
        });
        if !s.columns.iter().any(|(id, _)| id == "name") {
            s.columns.insert(0, ("name".into(), 300.));
        }
        normalize_volume_columns(&mut s.volume_columns);
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
