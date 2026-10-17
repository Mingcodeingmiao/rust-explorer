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
    failures: HashMap<IconRequest, (std::time::Instant, u8)>,
    retrying: std::collections::HashSet<IconRequest>,
    requests: mpsc::Sender<IconRequest>,
    results: mpsc::Receiver<(IconRequest, Option<PathBuf>)>,
    pixels: u32,
    dark: bool,
}
impl Icons {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<IconRequest>();
        let (results_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("system-icons".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch = vec![first];
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
            failures: HashMap::new(),
            retrying: Default::default(),
            requests: tx,
            results,
            pixels: 20,
            dark: true,
        }
    }
    fn get(&mut self, key: IconRequest) -> Option<PathBuf> {
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
        self.images.insert(key.clone(), previous.clone());
        let _ = self.requests.send(key);
        previous
    }
}
thread_local! {static ICONS:RefCell<Icons>=RefCell::new(Icons::new());}
pub fn set_pixels(pixels: u32) {
    ICONS.with(|v| v.borrow_mut().pixels = pixels.clamp(16, 96));
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
            let _ = v.requests.send(key);
        }
        changed
    })
}
pub fn icon(name: &str, color: u32) -> AnyElement {
    if matches!(name, "home" | "quick-access") {
        return drawn_icon(name, color);
    }
    let dark = ICONS.with(|v| v.borrow().dark);
    if let Some(path) = fluent_icons::path(name, dark) {
        if matches!(name, "close" | "pin" | "unpin") {
            return svg()
                .path(path.to_string_lossy().into_owned())
                .size(ui(16.))
                .flex_shrink_0()
                .text_color(rgb(color))
                .into_any_element();
        }
        return img(path)
            .size(ui(16.))
            .flex_shrink_0()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }
    if matches!(
        name,
        "new"
            | "new-item"
            | "new-folder"
            | "cut"
            | "copy"
            | "paste"
            | "rename"
            | "share"
            | "delete"
            | "sort"
            | "view"
            | "refresh"
            | "preview"
            | "close"
            | "pin"
            | "unpin"
            | "back"
            | "forward"
            | "up"
            | "search"
            | "new-tab"
            | "more"
            | "window-minimize"
            | "window-maximize"
            | "window-restore"
            | "chevron-down"
            | "chevron-right"
            | "chevron-left"
            | "check"
            | "checkbox"
    ) {
        return drawn_icon(name, color);
    }
    if matches!(
        name,
        "chevron-up" | "options" | "zip" | "details-pane" | "select-none" | "invert-selection"
    ) {
        return drawn_icon(
            match name {
                "chevron-up" => "chevron-up",
                "options" => "settings",
                "zip" => "archive",
                "details-pane" => "preview",
                _ => "select-all",
            },
            color,
        );
    }
    let path = ICONS.with(|v| {
        let mut v = v.borrow_mut();
        let key = IconRequest::ui(name, v.pixels);
        v.get(key)
    });
    picture_or_draw(path, name, color)
}
pub fn file_icon(path: &Path, directory: bool, color: u32) -> AnyElement {
    file_icon_sized(path, directory, color, 16.)
}
/// Badge for reparse links whose shared class icon has no Shell overlay.
/// Do not add to .lnk/.url icons: their per-item Shell bitmap owns its arrow.
pub fn link_badge(logical: f32) -> AnyElement {
    let edge = (logical * 0.36).clamp(8., 18.);
    div()
        .absolute()
        .left_0()
        .bottom_0()
        .size(ui(edge))
        .bg(rgb(0xffffff))
        .rounded(ui(1.))
        .border_1()
        .border_color(rgb(0xa0a0a0))
        .child(
            canvas(
                |_, _, _| (),
                |bounds, _, window, _| {
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);
                    let at =
                        |x: f32, y: f32| bounds.origin + point(px(w * x / 16.), px(h * y / 16.));
                    let mut arrow = PathBuilder::stroke(px((w / 8.).clamp(1., 2.)));
                    arrow.move_to(at(3., 13.));
                    arrow.line_to(at(3., 8.));
                    arrow.line_to(at(12., 8.));
                    arrow.move_to(at(8., 4.));
                    arrow.line_to(at(12., 8.));
                    arrow.line_to(at(8., 12.));
                    if let Ok(path) = arrow.build() {
                        window.paint_path(path, rgb(0x0067c0));
                    }
                },
            )
            .size_full(),
        )
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
    cache: &Entity<ThumbnailImages>,
    cx: &mut App,
) -> AnyElement {
    if let Some(thumbnail) = thumbnail {
        let scale = ICONS.with(|v| v.borrow().pixels as f32 / 16.);
        let edge =
            rust_explorer::icon_layout::thumbnail_edge(logical, dimensions.0, dimensions.1, scale);
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
            d.child(img(path).size_full().object_fit(ObjectFit::Contain))
        })
        .when(image.is_none(), |d| {
            d.child(drawn_icon(if directory { "folder" } else { "file" }, color))
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
            d.child(img(path).size_full().object_fit(ObjectFit::Contain))
        })
        .when(image.is_none(), |d| {
            d.child(file_icon_sized(path, false, color, logical))
        })
        .into_any_element()
}
fn picture_or_draw(path: Option<PathBuf>, name: &str, color: u32) -> AnyElement {
    if let Some(path) = path {
        img(path)
            .size(ui(16.))
            .flex_shrink_0()
            .object_fit(ObjectFit::Contain)
            .into_any_element()
    } else {
        drawn_icon(name, color)
    }
}
// Small vector fallbacks, used only until a native resource arrives or when
// no matching resource exists. They are painted by GPUI, not a browser/font.
fn drawn_icon(name: &str, color: u32) -> AnyElement {
    let paths: Vec<Vec<(f32, f32)>> = match name {
        "quick-access" => vec![vec![
            (8., 1.),
            (10., 6.),
            (15., 6.),
            (11., 10.),
            (13., 15.),
            (8., 12.),
            (3., 15.),
            (5., 10.),
            (1., 6.),
            (6., 6.),
            (8., 1.),
        ]],
        "new" | "new-item" => vec![
            vec![
                (8., 1.),
                (12., 2.),
                (15., 6.),
                (15., 10.),
                (12., 14.),
                (8., 15.),
                (4., 14.),
                (1., 10.),
                (1., 6.),
                (4., 2.),
                (8., 1.),
            ],
            vec![(8., 4.), (8., 12.)],
            vec![(4., 8.), (12., 8.)],
        ],
        "window-minimize" => vec![vec![(3., 8.), (13., 8.)]],
        "window-maximize" => vec![vec![(3., 3.), (13., 3.), (13., 13.), (3., 13.), (3., 3.)]],
        "window-restore" => vec![
            vec![(5., 3.), (13., 3.), (13., 11.)],
            vec![(3., 5.), (11., 5.), (11., 13.), (3., 13.), (3., 5.)],
        ],
        "chevron-down" => vec![vec![(4., 6.), (8., 10.), (12., 6.)]],
        "chevron-up" => vec![vec![(4., 10.), (8., 6.), (12., 10.)]],
        "chevron-right" => vec![vec![(6., 4.), (10., 8.), (6., 12.)]],
        "chevron-left" => vec![vec![(10., 4.), (6., 8.), (10., 12.)]],
        "check" => vec![vec![(3., 8.), (6., 11.), (13., 4.)]],
        "checkbox" => vec![vec![(2., 2.), (14., 2.), (14., 14.), (2., 14.), (2., 2.)]],
        "share" => vec![
            vec![(8., 3.), (13., 3.), (13., 8.)],
            vec![(6., 10.), (13., 3.)],
            vec![(6., 4.), (2., 4.), (2., 14.), (12., 14.), (12., 10.)],
        ],
        "more" => vec![
            vec![(2., 8.), (4., 8.)],
            vec![(7., 8.), (9., 8.)],
            vec![(12., 8.), (14., 8.)],
        ],
        "sort" => vec![
            vec![(3., 2.), (3., 14.)],
            vec![(1., 4.), (3., 2.), (5., 4.)],
            vec![(9., 2.), (9., 14.)],
            vec![(7., 12.), (9., 14.), (11., 12.)],
            vec![(12., 3.), (15., 3.)],
            vec![(12., 7.), (14., 7.)],
        ],
        "view" => vec![
            vec![(2., 3.), (14., 3.)],
            vec![(2., 8.), (14., 8.)],
            vec![(2., 13.), (14., 13.)],
        ],
        "pin" => vec![
            vec![(6., 2.), (12., 8.), (10., 10.), (4., 4.), (6., 2.)],
            vec![(4., 4.), (2., 8.), (8., 14.), (10., 10.)],
            vec![(5., 11.), (1., 15.)],
        ],
        "computer" => vec![
            vec![(1., 2.), (15., 2.), (15., 11.), (1., 11.), (1., 2.)],
            vec![(8., 11.), (8., 14.)],
            vec![(4., 14.), (12., 14.)],
        ],
        "network" => vec![
            vec![(5., 1.), (11., 1.), (11., 6.), (5., 6.), (5., 1.)],
            vec![(8., 6.), (8., 9.)],
            vec![(3., 12.), (3., 9.), (13., 9.), (13., 12.)],
            vec![(1., 12.), (5., 12.), (5., 15.), (1., 15.), (1., 12.)],
            vec![(11., 12.), (15., 12.), (15., 15.), (11., 15.), (11., 12.)],
        ],
        "view-grid" | "select-all" => vec![
            vec![(2., 2.), (6., 2.), (6., 6.), (2., 6.), (2., 2.)],
            vec![(10., 2.), (14., 2.), (14., 6.), (10., 6.), (10., 2.)],
            vec![(2., 10.), (6., 10.), (6., 14.), (2., 14.), (2., 10.)],
            vec![(10., 10.), (14., 10.), (14., 14.), (10., 14.), (10., 10.)],
        ],
        "view-list" | "view-details" | "view-content" => vec![
            vec![(2., 3.), (4., 3.)],
            vec![(6., 3.), (14., 3.)],
            vec![(2., 8.), (4., 8.)],
            vec![(6., 8.), (14., 8.)],
            vec![(2., 13.), (4., 13.)],
            vec![(6., 13.), (14., 13.)],
        ],
        "settings" => vec![
            vec![
                (6., 1.),
                (10., 1.),
                (10., 4.),
                (14., 4.),
                (15., 8.),
                (12., 10.),
                (12., 14.),
                (8., 15.),
                (5., 12.),
                (2., 12.),
                (1., 8.),
                (4., 5.),
                (4., 2.),
                (6., 1.),
            ],
            vec![(6., 6.), (10., 6.), (10., 10.), (6., 10.), (6., 6.)],
        ],
        "properties" => vec![
            vec![
                (3., 1.),
                (10., 1.),
                (14., 5.),
                (14., 15.),
                (3., 15.),
                (3., 1.),
            ],
            vec![(7., 7.), (10., 7.)],
            vec![(7., 10.), (10., 10.)],
            vec![(7., 13.), (10., 13.)],
        ],
        "eye" => vec![
            vec![
                (1., 8.),
                (5., 4.),
                (11., 4.),
                (15., 8.),
                (11., 12.),
                (5., 12.),
                (1., 8.),
            ],
            vec![(6., 6.), (10., 6.), (10., 10.), (6., 10.), (6., 6.)],
        ],
        "archive" => vec![
            vec![(2., 1.), (14., 1.), (14., 15.), (2., 15.), (2., 1.)],
            vec![(7., 2.), (9., 4.), (7., 6.), (9., 8.), (7., 10.), (9., 12.)],
        ],
        "shortcut" => vec![
            vec![(5., 2.), (14., 2.), (14., 13.), (8., 13.)],
            vec![(2., 14.), (2., 8.), (9., 8.)],
            vec![(6., 5.), (9., 8.), (6., 11.)],
        ],
        "back" => vec![
            vec![(10., 3.), (5., 8.), (10., 13.)],
            vec![(5., 8.), (14., 8.)],
        ],
        "forward" => vec![
            vec![(6., 3.), (11., 8.), (6., 13.)],
            vec![(2., 8.), (11., 8.)],
        ],
        "up" => vec![
            vec![(3., 7.), (8., 2.), (13., 7.)],
            vec![(8., 2.), (8., 14.)],
        ],
        "tabs" | "new-tab" | "plus" => vec![vec![(3., 8.), (13., 8.)], vec![(8., 3.), (8., 13.)]],
        "close" => vec![vec![(4., 4.), (12., 12.)], vec![(12., 4.), (4., 12.)]],
        "search" => vec![
            vec![
                (9., 2.),
                (4., 2.),
                (2., 4.),
                (2., 9.),
                (4., 11.),
                (9., 11.),
                (11., 9.),
                (11., 4.),
                (9., 2.),
            ],
            vec![(10., 10.), (14., 14.)],
        ],
        "refresh" => vec![
            vec![
                (13., 6.),
                (11., 3.),
                (5., 3.),
                (2., 6.),
                (2., 10.),
                (5., 13.),
                (11., 13.),
                (13., 10.),
            ],
            vec![(13., 2.), (13., 6.), (9., 6.)],
        ],
        "folder" | "new-folder" => vec![vec![
            (1., 4.),
            (6., 4.),
            (8., 6.),
            (15., 6.),
            (15., 13.),
            (1., 13.),
            (1., 4.),
        ]],
        "preview" => vec![
            vec![(1., 3.), (15., 3.), (15., 13.), (1., 13.), (1., 3.)],
            vec![(10., 3.), (10., 13.)],
        ],
        "home" => vec![
            vec![(1., 8.), (8., 2.), (15., 8.)],
            vec![(3., 7.), (3., 14.), (13., 14.), (13., 7.)],
        ],
        "delete" => vec![
            vec![(4., 5.), (4., 14.), (12., 14.), (12., 5.)],
            vec![(2., 4.), (14., 4.)],
            vec![(6., 4.), (6., 2.), (10., 2.), (10., 4.)],
        ],
        "copy" => vec![
            vec![(5., 5.), (14., 5.), (14., 14.), (5., 14.), (5., 5.)],
            vec![(11., 3.), (2., 3.), (2., 11.)],
        ],
        "cut" => vec![
            vec![
                (5., 8.),
                (3., 7.),
                (1., 8.),
                (1., 11.),
                (3., 12.),
                (5., 11.),
                (5., 8.),
            ],
            vec![
                (11., 8.),
                (13., 7.),
                (15., 8.),
                (15., 11.),
                (13., 12.),
                (11., 11.),
                (11., 8.),
            ],
            vec![(4., 10.), (12., 1.)],
            vec![(12., 10.), (4., 1.)],
        ],
        "rename" => vec![
            vec![(2., 5.), (5., 5.)],
            vec![(2., 11.), (5., 11.)],
            vec![(7., 2.), (10., 2.), (13., 2.)],
            vec![(10., 2.), (10., 14.)],
            vec![(7., 14.), (10., 14.), (13., 14.)],
        ],
        "desktop" => vec![
            vec![(1., 2.), (15., 2.), (15., 11.), (1., 11.), (1., 2.)],
            vec![(8., 11.), (8., 14.)],
            vec![(4., 14.), (12., 14.)],
        ],
        "downloads" => vec![
            vec![(8., 1.), (8., 11.)],
            vec![(4., 7.), (8., 11.), (12., 7.)],
            vec![(2., 11.), (2., 14.), (14., 14.), (14., 11.)],
        ],
        "pictures" => vec![
            vec![(1., 2.), (15., 2.), (15., 14.), (1., 14.), (1., 2.)],
            vec![(1., 12.), (6., 6.), (10., 10.), (12., 8.), (15., 12.)],
            vec![(11., 5.), (12., 5.)],
        ],
        "music" => vec![
            vec![(6., 12.), (6., 3.), (14., 1.), (14., 10.)],
            vec![
                (6., 11.),
                (3., 10.),
                (1., 12.),
                (2., 14.),
                (5., 14.),
                (6., 12.),
            ],
            vec![
                (14., 9.),
                (11., 8.),
                (9., 10.),
                (10., 12.),
                (13., 12.),
                (14., 10.),
            ],
        ],
        "videos" | "video" => vec![
            vec![(1., 2.), (15., 2.), (15., 14.), (1., 14.), (1., 2.)],
            vec![(4., 2.), (4., 14.)],
            vec![(12., 2.), (12., 14.)],
            vec![(6., 5.), (10., 8.), (6., 11.), (6., 5.)],
        ],
        "paste" => vec![
            vec![
                (5., 3.),
                (2., 3.),
                (2., 14.),
                (14., 14.),
                (14., 3.),
                (11., 3.),
            ],
            vec![(5., 2.), (11., 2.), (11., 5.), (5., 5.), (5., 2.)],
        ],
        "drive" => vec![
            vec![(3., 4.), (13., 4.), (15., 11.), (1., 11.), (3., 4.)],
            vec![(1., 11.), (1., 14.), (15., 14.), (15., 11.)],
        ],
        _ => vec![
            vec![
                (3., 1.),
                (10., 1.),
                (14., 5.),
                (14., 15.),
                (3., 15.),
                (3., 1.),
            ],
            vec![(10., 1.), (10., 5.), (14., 5.)],
        ],
    };
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale: f32 = bounds.size.width.into();
            let scale = scale / 16.;
            let mut builder = PathBuilder::stroke(px(1.3 * scale));
            for line in &paths {
                for (i, (x, y)) in line.iter().enumerate() {
                    let p = bounds.origin + point(px(*x * scale), px(*y * scale));
                    if i == 0 {
                        builder.move_to(p);
                    } else {
                        builder.line_to(p);
                    }
                }
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .size(ui(16.))
    .flex_shrink_0()
    .into_any_element()
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
