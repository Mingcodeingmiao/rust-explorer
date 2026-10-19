//! Runtime references to the installed Windows 11 File Explorer SVG assets.
//!
//! No Windows artwork is embedded, copied or written to the application cache.
//! These private package paths can change with Windows updates, so callers must
//! use the embedded Microsoft Fluent assets when `path` returns `None`.
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

const PACKAGE: &str = "MicrosoftWindows.Client.FileExp_cw5n1h2txyewy";

fn mappings() -> &'static [(&'static str, &'static str)] {
    &[
        ("new", "windows.newitem.svg"),
        ("new-item", "windows.newitem.svg"),
        ("new-folder", "newfolder.svg"),
        ("cut", "windows.cut.svg"),
        ("copy", "windows.copy.svg"),
        ("paste", "windows.paste.svg"),
        ("rename", "windows.rename.svg"),
        ("share", "Windows.ModernShare.svg"),
        ("delete", "windows.ribbondelete.svg"),
        ("sort", "SortBy.svg"),
        ("view", "View.svg"),
        ("refresh", "windows.refresh.svg"),
        ("preview", "windows.previewpane.svg"),
        ("details-pane", "windows.readingpane.svg"),
        ("close", "windows.close.svg"),
        ("pin", "windows.pintohome.svg"),
        ("unpin", "windows.unpinfromhome.svg"),
        // This shipped asset is the Quick Access star, not the Home house.
        ("quick-access", "addressbar.home.svg"),
        ("computer", "addressbar.desktop.svg"),
        ("network", "addressbar.network.svg"),
        ("gallery", "addressbar.gallery.svg"),
        ("properties", "windows.properties.svg"),
        ("options", "windows.folderoptions.svg"),
        ("copy-path", "windows.copyaspath.svg"),
        ("select-all", "windows.selectall.svg"),
        ("select-none", "windows.selectnone.svg"),
        ("invert-selection", "windows.invertselection.svg"),
        ("zip", "Windows.Zip.Action.svg"),
        ("large-icons", "windows.iconsize.large.svg"),
        ("extra-large-icons", "windows.iconsize.extralarge.svg"),
        ("medium-icons", "windows.iconsize.medium.svg"),
        ("small-icons", "windows.iconsize.smallicon.svg"),
        ("details", "windows.iconsize.details.svg"),
        ("list", "windows.iconsize.list.svg"),
        ("tiles", "windows.iconsize.tile.svg"),
        ("content", "windows.iconsize.content.svg"),
    ]
}

struct Catalog {
    paths: HashMap<(&'static str, bool), PathBuf>,
    source: Option<PathBuf>,
    modules: Vec<PathBuf>,
    epoch: u64,
}

fn config_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustExplorer/resource-source.json")
}
fn catalog() -> &'static RwLock<Catalog> {
    static CATALOG: OnceLock<RwLock<Catalog>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let source: Option<PathBuf> = std::fs::read(config_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        RwLock::new(discover(source))
    })
}

fn discover(source: Option<PathBuf>) -> Catalog {
    let mut windows = [0_u16; 32768];
    let count = unsafe {
        windows::Win32::System::SystemInformation::GetWindowsDirectoryW(Some(&mut windows))
    } as usize;
    let mut paths = HashMap::new();
    let mut roots = Vec::new();
    if count > 0 && count < windows.len() {
        roots.push(
            PathBuf::from(String::from_utf16_lossy(&windows[..count]))
                .join("SystemApps")
                .join(PACKAGE)
                .join("FileExplorerExtensions/Assets/images/contrast-standard"),
        );
    }
    let mut modules = Vec::new();
    if let Some(source) = &source {
        let root = if source.is_dir() {
            source.as_path()
        } else {
            source.parent().unwrap_or(source)
        };
        for candidate in [
            root.to_path_buf(),
            root.join("Assets/images/contrast-standard"),
            root.join("FileExplorerExtensions/Assets/images/contrast-standard"),
            root.join(PACKAGE)
                .join("FileExplorerExtensions/Assets/images/contrast-standard"),
        ] {
            roots.push(candidate);
        }
        for dll in [
            "shell32.dll",
            "imageres.dll",
            "shell32.dll.mun",
            "imageres.dll.mun",
        ] {
            for candidate in [
                root.join(dll),
                root.join("System32").join(dll),
                root.join("SystemResources").join(dll),
            ] {
                if candidate.is_file() && !modules.contains(&candidate) {
                    modules.push(candidate);
                }
            }
        }
        if source.is_file() && !modules.contains(source) {
            modules.push(source.clone());
        }
    }
    for root in roots {
        for dark in [false, true] {
            for &(name, filename) in mappings() {
                let themed = root
                    .join(if dark { "theme-dark" } else { "theme-light" })
                    .join(filename);
                let plain = root.join(filename);
                if let Some(path) = [themed, plain].into_iter().find(|p| p.is_file()) {
                    paths.insert((name, dark), path);
                }
            }
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hash);
    for module in &modules {
        module.hash(&mut hash);
        if let Ok(metadata) = std::fs::metadata(module) {
            metadata.len().hash(&mut hash);
            metadata.modified().ok().hash(&mut hash);
        }
    }
    let epoch = if source.is_some() {
        hash.finish().max(1)
    } else {
        0
    };
    Catalog {
        paths,
        source,
        modules,
        epoch,
    }
}

#[derive(Clone, Debug)]
pub struct ResourceStatus {
    pub source: Option<PathBuf>,
    pub svg_count: usize,
    /// Only modules with known filename/resource mappings can supply native icons.
    pub native_module_count: usize,
}
fn status_of(catalog: &Catalog) -> ResourceStatus {
    ResourceStatus {
        source: catalog.source.clone(),
        svg_count: catalog.paths.len(),
        native_module_count: catalog
            .modules
            .iter()
            .filter(|path| is_mapped_module(path))
            .count(),
    }
}
fn is_mapped_module(path: &Path) -> bool {
    matches!(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str(),
        "shell32.dll" | "imageres.dll" | "shell32.dll.mun" | "imageres.dll.mun"
    )
}
/// Persist a reference, never copy artwork or execute a selected library.
/// Run discovery on a background task. SVG paths and subsequent icon requests
/// use the new catalog immediately; already displayed icons may finish loading.
pub fn set_resource_source(source: Option<PathBuf>) -> Result<ResourceStatus> {
    if let Some(path) = &source {
        ensure!(
            path.is_absolute() && path.exists(),
            "请选择存在的绝对资源路径"
        );
        ensure!(
            path.is_dir()
                || matches!(
                    path.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "dll" | "exe" | "mun"
                ),
            "请选择资源目录、DLL、EXE 或 MUN 文件"
        );
    }
    let next = discover(source.clone());
    if source.is_some() {
        ensure!(
            !next.modules.is_empty()
                || next
                    .paths
                    .iter()
                    .any(|(_, p)| source
                        .as_ref()
                        .is_some_and(|s| p.starts_with(if s.is_dir() {
                            s.as_path()
                        } else {
                            s.parent().unwrap_or(s)
                        }))),
            "未找到可识别的资源文件或 Explorer SVG"
        );
    }
    let config = config_path();
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config, serde_json::to_vec(&source)?)?;
    let status = status_of(&next);
    *catalog().write().unwrap() = next;
    Ok(status)
}
pub fn resource_status() -> ResourceStatus {
    status_of(&catalog().read().unwrap())
}
pub fn resource_source() -> Option<PathBuf> {
    catalog().read().unwrap().source.clone()
}
pub fn resource_epoch() -> u64 {
    catalog().read().unwrap().epoch
}
/// Only exact verified module names participate. A random EXE's numeric IDs are
/// unrelated to Shell's; choosing it cannot reproduce an entire Windows theme.
pub fn resource_module(dll: &str) -> Option<PathBuf> {
    catalog()
        .read()
        .unwrap()
        .modules
        .iter()
        .find(|path| {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            file.eq_ignore_ascii_case(dll) || file.eq_ignore_ascii_case(&format!("{dll}.mun"))
        })
        .cloned()
}

/// Call once before opening windows to keep filesystem discovery off rendering.
pub fn initialize() {
    let _ = catalog();
}

/// Return an installed Explorer asset, never a generated or redistributed copy.
/// `back`, `forward`, `up`, `search`, `plus` and `tabs` have no semantically exact
/// SVG in the inspected package and deliberately return `None`.
pub fn path(name: &str, dark: bool) -> Option<PathBuf> {
    catalog().read().unwrap().paths.get(&(name, dark)).cloned()
}

/// Optional alpha-mask loading for GPUI `svg().path(...)`. For original colors,
/// use `img(path(...))`: GPUI's image loader reads and renders SVG asynchronously.
#[cfg(feature = "desktop")]
pub struct Assets;

#[cfg(feature = "desktop")]
impl gpui::AssetSource for Assets {
    fn load(&self, requested: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(name) = requested
            .strip_prefix("fluent/")
            .and_then(|p| p.strip_suffix(".svg"))
        {
            return Ok(crate::official_icons::bytes(name).map(std::borrow::Cow::Borrowed));
        }
        let requested = std::path::Path::new(requested);
        if !catalog()
            .read()
            .unwrap()
            .paths
            .values()
            .any(|known| known == requested)
        {
            return Ok(None);
        }
        match std::fs::read(requested) {
            Ok(bytes) if bytes.len() <= 256 * 1024 => Ok(Some(bytes.into())),
            _ => Ok(None),
        }
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_assets_override_installed_paths_without_copying() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "rust-explorer-resource-test-{}",
            std::process::id()
        ));
        let dark = root.join("Assets/images/contrast-standard/theme-dark");
        std::fs::create_dir_all(&dark)?;
        let asset = dark.join("windows.copy.svg");
        std::fs::write(&asset, b"<svg xmlns='http://www.w3.org/2000/svg'/>")?;
        let discovered = discover(Some(root.clone()));
        assert_eq!(discovered.paths.get(&("copy", true)), Some(&asset));
        assert!(discovered.epoch != 0);
        assert!(!is_mapped_module(Path::new("unrelated.exe")));
        assert!(is_mapped_module(Path::new("IMAGERES.DLL.MUN")));
        std::fs::remove_file(asset)?;
        std::fs::remove_dir(dark)?;
        std::fs::remove_dir(root.join("Assets/images/contrast-standard"))?;
        std::fs::remove_dir(root.join("Assets/images"))?;
        std::fs::remove_dir(root.join("Assets"))?;
        std::fs::remove_dir(root)?;
        Ok(())
    }
}
