use gpui::prelude::*;
use gpui::*;
use rust_explorer::fluent_icons;
use rust_explorer::native_assets::{self, IconRequest};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::mpsc,
};

pub fn ui(pixels: f32) -> Rems {
    rems(pixels / 16.)
}
/// Unlike GPUI's global retain-all cache, thumbnail textures have a fixed budget.
pub struct ThumbnailImages {
    inner: Entity<RetainAllImageCache>,
    used: HashMap<Resource, (u64, usize)>,
    clock: u64,
}
impl ThumbnailImages {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let inner = RetainAllImageCache::new(cx);
        cx.new(|_| Self {
            inner,
            used: HashMap::new(),
            clock: 0,
        })
    }
}
impl ImageCache for ThumbnailImages {
    fn load(
        &mut self,
        source: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<std::sync::Arc<RenderImage>, ImageCacheError>> {
        let result = self
            .inner
            .update(cx, |cache, cx| cache.load(source, window, cx));
        self.clock += 1;
        let bytes = match &result {
            Some(Ok(image)) => {
                let size = image.size(0);
                size.width.0 as usize * size.height.0 as usize * 4
            }
            _ => self
                .used
                .get(source)
                .map(|(_, bytes)| *bytes)
                .unwrap_or(256 * 256 * 4),
        };
        self.used.insert(source.clone(), (self.clock, bytes));
        let mut total: usize = self.used.values().map(|(_, bytes)| *bytes).sum();
        while self.used.len() > 512 || total > 32 * 1024 * 1024 {
            let Some(oldest) = self
                .used
                .iter()
                .filter(|(key, _)| *key != source)
                .min_by_key(|(_, (used, _))| *used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((_, bytes)) = self.used.remove(&oldest) {
                total = total.saturating_sub(bytes);
            }
            self.inner
                .update(cx, |cache, cx| cache.remove(&oldest, window, cx));
        }
        result
    }
}
struct Icons {
    images: HashMap<IconRequest, Option<PathBuf>>,
    /// Last resolved picture per (path, directory, pixels). A volume refresh bumps
    /// the drive-key epoch, and without this the item would fall back to the drawn
    /// folder glyph for a frame or two until the refreshed icon arrives.
    last: HashMap<(PathBuf, bool, u32), PathBuf>,
    failures: HashMap<IconRequest, (std::time::Instant, u8)>,
    retrying: std::collections::HashSet<IconRequest>,
    requests: mpsc::Sender<IconRequest>,
    priority: mpsc::Sender<IconRequest>,
    results: mpsc::Receiver<(IconRequest, Option<PathBuf>)>,
    pixels: u32,
    dark: bool,
    generation: u64,
}
impl Icons {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<IconRequest>();
        let (results_tx, results) = mpsc::channel();
        let (priority, urgent) = mpsc::channel::<IconRequest>();
        let urgent_results = results_tx.clone();
        std::thread::Builder::new()
            .name("navigation-icons".into())
            .spawn(move || {
                while let Ok(first) = urgent.recv() {
                    let mut batch = vec![first];
                    // Larger batches mean fewer helper starts while scrolling a big
                    // folder; the helper deadline scales with the batch.
                    batch.extend(urgent.try_iter().take(31));
                    let paths = native_assets::icons(batch.clone());
                    for (key, path) in batch.into_iter().zip(paths) {
                        if urgent_results.send((key, path)).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("navigation icon worker");
        std::thread::Builder::new()
            .name("system-icons".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch = vec![first];
                    // Larger batches keep cold-start Shell work in one helper call
                    // instead of one process start per eight icons.
                    batch.extend(rx.try_iter().take(31));
                    let paths = native_assets::icons(batch.clone());
                    for (key, path) in batch.into_iter().zip(paths) {
                        if results_tx.send((key, path)).is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("system icon worker");
        Self {
            images: HashMap::new(),
            last: HashMap::new(),
            failures: HashMap::new(),
            retrying: Default::default(),
            requests: tx,
            priority,
            results,
            pixels: 20,
            dark: true,
            generation: 0,
        }
    }
    fn get(&mut self, mut key: IconRequest) -> Option<PathBuf> {
        key.epoch = key.epoch.wrapping_add(self.generation << 32);
        if self.images.len() >= 2048 && !self.images.contains_key(&key) {
            self.images.clear();
            self.failures.clear();
            self.retrying.clear();
        }
        let previous = if key.epoch != 0 {
            self.images.iter().find_map(|(old, path)| {
                if old.path == key.path && old.pixels == key.pixels && old.name == key.name {
                    path.clone()
                } else {
                    None
                }
            })
        } else {
            None
        };
        // A refreshed (new-epoch) request must not drop the icon that is already on
        // screen; keep the last resolved picture for this path and size.
        let previous = previous.or_else(|| {
            self.last
                .get(&(key.path.clone(), key.directory, key.pixels))
                .cloned()
        });
        if key.epoch != 0 {
            self.images.retain(|old, _| {
                old.path != key.path
                    || old.name != key.name
                    || old.pixels != key.pixels
                    || old.epoch == key.epoch
            });
        }
        if let Some(path) = self.images.get(&key) {
            return path.clone();
        }
        // The icon may already be on disk from an earlier session. Reading the small
        // PNG path here (hash + existence check, ~10 µs) paints it on the first frame
        // instead of waiting for a helper round trip.
        if let Some(path) = native_assets::cached_icon_path(&key) {
            self.images.insert(key, Some(path.clone()));
            return Some(path);
        }
        self.images.insert(key.clone(), previous.clone());
        self.request(key);
        previous
    }
    /// Queue icons for the whole folder without blocking: used to have every
    /// executable/shortcut icon ready before the user scrolls to it.
    fn prefetch(&mut self, keys: Vec<IconRequest>) {
        let mut pending = Vec::new();
        for key in keys {
            if self.images.contains_key(&key) || self.failures.contains_key(&key) {
                continue;
            }
            // Revisits are already on disk: take them without spawning a helper.
            if let Some(path) = native_assets::cached_icon_path(&key) {
                self.images.insert(key, Some(path));
            } else {
                pending.push(key);
            }
        }
        for key in pending {
            if self.images.contains_key(&key) {
                continue;
            }
            self.images.insert(key.clone(), None);
            self.request(key);
        }
    }
    fn request(&self, key: IconRequest) {
        let priority = key.name != "file"
            || (key.directory && key.path.has_root() && key.path.parent().is_none());
        let _ = if priority {
            self.priority.send(key)
        } else {
            self.requests.send(key)
        };
    }
}
thread_local! {static ICONS:RefCell<Icons>=RefCell::new(Icons::new());}
thread_local! {static ICON_IMAGES:RefCell<Option<Entity<ThumbnailImages>>> = const { RefCell::new(None) };}
pub fn initialize_image_cache(cx: &mut App) {
    ICON_IMAGES.with(|cache| {
        if cache.borrow().is_none() {
            *cache.borrow_mut() = Some(ThumbnailImages::new(cx));
        }
    });
}
pub fn reset_image_cache(cx: &mut App) {
    ICON_IMAGES.with(|cache| *cache.borrow_mut() = Some(ThumbnailImages::new(cx)));
}
fn icon_image(path: PathBuf) -> Img {
    let image = img(path);
    if let Some(cache) = ICON_IMAGES.with(|cache| cache.borrow().clone()) {
        image.image_cache(&cache)
    } else {
        image
    }
}
pub fn clear_cache() {
    ICONS.with(|icons| {
        let mut icons = icons.borrow_mut();
        icons.images.clear();
        icons.last.clear();
        icons.failures.clear();
        icons.retrying.clear();
        icons.generation = icons.generation.wrapping_add(1);
    });
}
/// Resolve these icons in the background. Visible rows still jump the queue, but a
/// folder's executables and shortcuts are ready before they are scrolled to.
pub fn prefetch_icons(keys: Vec<IconRequest>) {
    ICONS.with(|icons| icons.borrow_mut().prefetch(keys));
}
pub fn set_pixels(pixels: u32) {
    ICONS.with(|v| v.borrow_mut().pixels = pixels.clamp(16, 96));
}
/// Current icon pixel size (zoom/view dependent).
pub fn icon_pixels() -> u32 {
    ICONS.with(|v| v.borrow().pixels)
}
pub fn set_theme(dark: bool) {
    ICONS.with(|v| v.borrow_mut().dark = dark);
}
pub fn checkbox(checked: bool, color: u32) -> AnyElement {
    div()
        .size(ui(16.))
        .flex_shrink_0()
        .rounded(ui(3.))
        .border_1()
        .border_color(rgb(color))
        .when(checked, |d| d.child(drawn_icon("check", color)))
        .into_any_element()
}
pub fn drain() -> bool {
    ICONS.with(|v| {
        let mut v = v.borrow_mut();
        let results: Vec<_> = v.results.try_iter().collect();
        let changed = results.iter().any(|(_, p)| p.is_some());
        for (k, p) in results {
            v.retrying.remove(&k);
            if p.is_none() {
                let count = v
                    .failures
                    .get(&k)
                    .map_or(1, |(_, count)| count.saturating_add(1));
                v.failures
                    .insert(k.clone(), (std::time::Instant::now(), count));
            } else {
                v.failures.remove(&k);
            }
            if let Some(path) = &p {
                v.last
                    .insert((k.path.clone(), k.directory, k.pixels), path.clone());
                if v.last.len() > 4096 {
                    v.last.clear();
                }
            }
            v.images.insert(k, p);
        }
        let retry: Vec<_> = v
            .failures
            .iter()
            .filter(|(key, (when, count))| {
                *count < 3
                    && when.elapsed() >= std::time::Duration::from_secs(2)
                    && !v.retrying.contains(*key)
            })
            .map(|(key, _)| key.clone())
            .take(32)
            .collect();
        for key in retry {
            v.retrying.insert(key.clone());
            v.request(key);
        }
        changed
    })
}
pub fn icon_at(name: &str, color: u32, logical: f32) -> AnyElement {
    icon_sized(name, color, logical)
}
pub fn icon(name: &str, color: u32) -> AnyElement {
    icon_sized(name, color, if name == "pin" { 18. * 0.75 } else { 16. })
}
pub fn icon_sized(name: &str, color: u32, logical: f32) -> AnyElement {
    // Never use the legacy red-X icon or Win10 toolbar raster resources.
    if name == "close"
        || (!rust_explorer::system_icons::windows_11()
            && matches!(
                name,
                "cut" | "copy" | "paste" | "rename" | "share" | "delete"
            ))
    {
        return drawn_icon_sized(name, color, logical);
    }
    if let Some((font, glyph)) = system_glyph(name) {
        return div()
            .size(ui(logical))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .font_family(font)
            .text_size(ui(logical))
            .line_height(ui(logical))
            .text_color(rgb(color))
            .child(glyph.to_string())
            .into_any_element();
    }
    let dark = ICONS.with(|v| v.borrow().dark);
    if let Some(path) = fluent_icons::path(name, dark).filter(|_| {
        !matches!(
            name,
            "network"
                | "computer"
                | "computer-colored"
                | "pin"
                | "home"
                | "desktop"
                | "downloads"
                | "documents"
                | "pictures"
                | "music"
                | "videos"
        )
    }) {
        if matches!(name, "close" | "pin" | "unpin") {
            return svg()
                .path(path.to_string_lossy().into_owned())
                .size(ui(logical))
                .flex_shrink_0()
                .text_color(rgb(color))
                .into_any_element();
        }
        return img(path)
            .size(ui(logical))
            .flex_shrink_0()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }
    if !rust_explorer::system_icons::has_ui_icon(name) {
        return drawn_icon_sized(name, color, logical);
    }
    let path = ICONS.with(|v| {
        let mut v = v.borrow_mut();
        let key = IconRequest::ui(name, (v.pixels as f32 * logical / 16.).ceil() as u32);
        v.get(key)
    });
    picture_or_draw_sized(path, name, color, logical)
}
pub fn file_icon(path: &Path, directory: bool, color: u32) -> AnyElement {
    file_icon_sized(path, directory, color, 16.)
}
/// Badge for reparse links whose shared class icon has no Shell overlay.
/// Do not add to .lnk/.url icons: their per-item Shell bitmap owns its arrow.
///
/// The Shell exposes the link overlay only as part of a complete icon (there is no
/// standalone overlay bitmap API), so the arrow stays drawn; its size follows the
/// icon size the way Explorer's overlay does.
pub fn link_badge(logical: f32) -> AnyElement {
    // One badge per item, sized from the icon: a fixed 16 px box looked oversized
    // next to extra-large icons and tiny next to medium ones.
    let edge = if logical >= 140. {
        logical * 0.20
    } else if logical >= 96. {
        logical * 0.20
    } else if logical >= 56. {
        logical * 0.20
    } else {
        (logical * 0.36).clamp(8., 18.)
    };
    div()
        .absolute()
        .left_0()
        .bottom_0()
        .size(ui(edge))
        .bg(rgb(0xffffff))
        .rounded(ui(1.))
        .border_1()
        .border_color(rgb(0xa0a0a0))
        .child(drawn_icon_sized("shortcut", 0x0067c0, edge))
        .into_any_element()
}
pub fn entry_icon(
    path: &Path,
    directory: bool,
    link: bool,
    color: u32,
    logical: f32,
) -> AnyElement {
    div()
        .size(ui(logical))
        .flex_shrink_0()
        .relative()
        .child(file_icon_sized(path, directory, color, logical))
        .when(link && !rust_explorer::preview::is_shortcut(path), |d| {
            d.child(link_badge(logical))
        })
        .into_any_element()
}
pub fn thumbnail_pixels(logical: f32) -> u32 {
    ICONS.with(|v| {
        rust_explorer::thumbnails::resolution(
            (v.borrow().pixels as f32 * logical / 16.).ceil() as u32
        )
    })
}
pub fn thumbnail_or_icon(
    path: &Path,
    directory: bool,
    color: u32,
    logical: f32,
    thumbnail: Option<PathBuf>,
    dimensions: (u32, u32),
    folder_thumbnail: bool,
    cache: &Entity<ThumbnailImages>,
    cx: &mut App,
) -> AnyElement {
    if let Some(thumbnail) = thumbnail {
        let scale = ICONS.with(|v| v.borrow().pixels as f32 / 16.);
        let edge = rust_explorer::icon_layout::thumbnail_edge(
            logical,
            dimensions.0,
            dimensions.1,
            scale,
            folder_thumbnail,
        );
        let long = dimensions.0.max(dimensions.1).max(1) as f32;
        let (image_width, image_height) = if dimensions.0 > 0 && dimensions.1 > 0 {
            (
                edge * dimensions.0 as f32 / long,
                edge * dimensions.1 as f32 / long,
            )
        } else {
            (edge, edge)
        };
        let resource = Resource::from(thumbnail.clone());
        let pixels = thumbnail_pixels(logical) as usize;
        cache.update(cx, |cache, _| {
            cache
                .used
                .entry(resource)
                .or_insert((cache.clock, pixels * pixels * 4));
        });
        div()
            .size(ui(logical))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(
                div()
                    .size(ui(edge))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(edge + 1. < logical, |d| {
                        d.border_1().border_color(rgba(0x808080a0))
                    })
                    .child(
                        img(thumbnail)
                            .image_cache(cache)
                            .w(ui(image_width))
                            .h(ui(image_height))
                            .flex_shrink_0()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    } else {
        file_icon_sized(path, directory, color, logical)
    }
}
pub fn focus_outline(color: u32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let mut b = PathBuilder::stroke(px(1.));
            let w = f32::from(bounds.size.width) - 2.;
            let h = f32::from(bounds.size.height) - 2.;
            for i in (0..w.max(0.) as usize).step_by(4) {
                for y in [1., h] {
                    b.move_to(bounds.origin + point(px(i as f32 + 1.), px(y)));
                    b.line_to(bounds.origin + point(px((i as f32 + 3.).min(w)), px(y)));
                }
            }
            for i in (0..h.max(0.) as usize).step_by(4) {
                for x in [1., w] {
                    b.move_to(bounds.origin + point(px(x), px(i as f32 + 1.)));
                    b.line_to(bounds.origin + point(px(x), px((i as f32 + 3.).min(h))));
                }
            }
            if let Ok(path) = b.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
pub fn film_strip() -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let rail = (width * 0.09).clamp(4., 9.);
            for x in [0., width - rail] {
                window.paint_quad(fill(
                    Bounds::new(
                        bounds.origin + point(px(x), px(0.)),
                        size(px(rail), px(height)),
                    ),
                    rgb(0x252525),
                ));
                for y in (3..height.max(3.) as usize).step_by(8) {
                    window.paint_quad(fill(
                        Bounds::new(
                            bounds.origin + point(px(x + 1.), px(y as f32)),
                            size(px((rail - 2.).max(2.)), px(4.)),
                        ),
                        rgb(0x888888),
                    ));
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
pub fn file_icon_sized(path: &Path, directory: bool, color: u32, logical: f32) -> AnyElement {
    let image = ICONS.with(|v| {
        let mut v = v.borrow_mut();
        let key = IconRequest::file(
            path,
            directory,
            (v.pixels as f32 * logical / 16.).ceil().clamp(8., 256.) as u32,
        );
        v.get(key)
    });
    div()
        .size(ui(logical))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .when_some(image.clone(), |d, path| {
            d.child(icon_image(path).size_full().object_fit(ObjectFit::Contain))
        })
        .when(image.is_none(), |d| {
            d.child(drawn_icon_sized(
                if directory && path.has_root() && path.parent().is_none() {
                    "drive"
                } else if directory {
                    "folder"
                } else {
                    "file"
                },
                color,
                logical,
            ))
        })
        .into_any_element()
}

pub fn associated_app_icon(path: &Path, color: u32, logical: f32) -> AnyElement {
    let image = ICONS.with(|icons| {
        let mut icons = icons.borrow_mut();
        let mut request = IconRequest::file(
            path,
            false,
            (icons.pixels as f32 * logical / 16.).ceil().clamp(8., 256.) as u32,
        );
        request.name = "associated-app".into();
        // Refresh association changes without adding a request for every file.
        request.epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 60;
        icons.get(request)
    });
    div()
        .size(ui(logical))
        .flex_shrink_0()
        .when_some(image.clone(), |d, path| {
            d.child(icon_image(path).size_full().object_fit(ObjectFit::Contain))
        })
        .when(image.is_none(), |d| {
            d.child(file_icon_sized(path, false, color, logical))
        })
        .into_any_element()
}
fn picture_or_draw_sized(
    path: Option<PathBuf>,
    name: &str,
    color: u32,
    logical: f32,
) -> AnyElement {
    if let Some(path) = path {
        icon_image(path)
            .size(ui(logical))
            .flex_shrink_0()
            .object_fit(ObjectFit::Contain)
            .into_any_element()
    } else {
        drawn_icon_sized(name, color, logical)
    }
}
fn drawn_icon(name: &str, color: u32) -> AnyElement {
    drawn_icon_sized(name, color, 16.)
}
// Official Microsoft SVGs are embedded, so missing Windows assets never require
// network access or a hand-drawn substitute.
fn drawn_icon_sized(name: &str, color: u32, logical: f32) -> AnyElement {
    let name = if name == "address-computer" {
        "computer-outline"
    } else {
        name
    };
    let name = if rust_explorer::official_icons::bytes(name).is_some() {
        name
    } else {
        "file"
    };
    svg()
        .path(format!("fluent/{name}.svg"))
        .size(ui(logical))
        .flex_shrink_0()
        .text_color(rgb(color))
        .into_any_element()
}
fn system_glyph(name: &str) -> Option<(&'static str, char)> {
    static FONT: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    let font = *FONT.get_or_init(|| {
        let fonts = PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts");
        if fonts.join("SegoeIcons.ttf").is_file() {
            Some("Segoe Fluent Icons")
        } else if fonts.join("segmdl2.ttf").is_file() {
            Some("Segoe MDL2 Assets")
        } else {
            None
        }
    });
    let glyph = match name {
        "back" => '\u{e72b}',
        "forward" => '\u{e72a}',
        "up" => '\u{e74a}',
        "refresh" => '\u{e72c}',
        "address-computer" => '\u{e7f4}',
        "search" => '\u{e721}',
        _ => return None,
    };
    Some((font?, glyph))
}

pub fn button_name(label: &str) -> Option<&'static str> {
    if label.starts_with("本地磁盘") {
        return Some("drive");
    }
    Some(match label {
        "←" => "back",
        "→" => "forward",
        "↑" => "up",
        "⟳" => "refresh",
        "＋" => "new-tab",
        "×" => "close",
        "‹" => "chevron-left",
        "›" => "chevron-right",
        "新建" | "⊕ 新建" => "new",
        "剪切" => "cut",
        "复制" => "copy",
        "粘贴" => "paste",
        "重命名" => "rename",
        "共享" => "share",
        "删除" => "delete",
        "▣ 预览" | "□ 预览" | "预览" => "preview",
        "⌂ 主文件夹" | "主文件夹" => "home",
        "桌面" => "desktop",
        "下载" => "downloads",
        "文档" => "documents",
        "图片" => "pictures",
        "音乐" => "music",
        "视频" => "videos",
        _ => return None,
    })
}
