//! Independent window listing running and recent system operations.
//!
//! The status bar used to open a centered popup with two lines per operation.
//! A separate window can show the details, keep the history after the popup
//! would have closed, and stay open next to the folder window.
use super::*;

pub(super) struct OperationsWindow {
    theme: Theme,
    zoom: f32,
    /// Last observed `operations::version()`, so the poll only re-renders on change.
    version: u64,
    scroll: ScrollHandle,
}

impl OperationsWindow {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let theme = Theme::preference(&settings.theme);
        let zoom = 1.;
        // The records live in a process-wide store: poll its version so the
        // window follows operations started from any folder window.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let updated = this.update(cx, |this, cx| {
                    let version = rust_explorer::operations::version();
                    if version != this.version {
                        this.version = version;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            theme,
            zoom,
            version: rust_explorer::operations::version(),
            scroll: ScrollHandle::new(),
        }
    }
}

impl Render for OperationsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        window.set_rem_size(px(16. * self.zoom));
        visual::set_theme(t.dark);
        let records = rust_explorer::operations::snapshot();
        let running = records
            .iter()
            .filter(|record| record.finished.is_none())
            .count();
        let mut list = div()
            .id("operation-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p_3()
            .flex()
            .flex_col()
            .gap_2();
        if records.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_color(rgb(t.muted))
                    .child(rust_explorer::i18n::text("暂无系统操作记录")),
            );
        }
        // Running operations come first, newest first inside each group.
        let mut ordered = records.clone();
        ordered.sort_by(|a, b| {
            b.finished
                .is_none()
                .cmp(&a.finished.is_none())
                .then_with(|| b.started.cmp(&a.started))
        });
        for record in &ordered {
            list = list.child(operation_card(&t, record, cx));
        }
        div()
            .size_full()
            .bg(rgb(t.chrome))
            .text_color(rgb(t.text))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(ui(40.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        div().flex().items_center().gap_2().child(
                            div()
                                .text_size(ui(14.))
                                .child(rust_explorer::i18n::text("系统操作详情")),
                        ),
                    )
                    .when(running > 0, |d| {
                        d.child(
                            button("operations-stop-all", rust_explorer::i18n::text("停止"), t)
                                .on_click(cx.listener(|_, _, _, cx| {
                                    rust_explorer::operations::stop_all(false);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(list)
    }
}

/// One operation: label and status, how long it took, what it acts on, and the
/// stop controls while it is running.
fn operation_card(
    t: &Theme,
    record: &rust_explorer::operations::OperationRecord,
    cx: &mut Context<OperationsWindow>,
) -> AnyElement {
    let finished = record.finished.is_some();
    let elapsed = record
        .finished
        .unwrap_or_else(Instant::now)
        .duration_since(record.started)
        .as_secs_f32();
    let id = record.id;
    let mut card = div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded(ui(6.))
        .bg(rgb(t.panel))
        .border_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(record.label.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(if finished { t.muted } else { t.accent }))
                        .child(record.status.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_color(rgb(t.muted))
                .child(format!("{} · ", record.started_at.format("%H:%M:%S")))
                .child(rust_explorer::trf!("用时：{elapsed:.1} 秒")),
        );
    if !record.details.is_empty() {
        card = card.child(
            div()
                .text_color(rgb(t.muted))
                .whitespace_normal()
                .child(record.details.clone()),
        );
    }
    if !finished {
        card = card.child(
            div()
                .flex()
                .gap_2()
                .child(
                    button(
                        ("operation-stop", id),
                        rust_explorer::i18n::text("停止"),
                        *t,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        rust_explorer::operations::stop(id, false);
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        ("operation-force", id),
                        rust_explorer::i18n::text("强行停止"),
                        *t,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        rust_explorer::operations::stop(id, true);
                        cx.notify();
                    })),
                ),
        );
    }
    card.into_any_element()
}

/// Open (or focus) the operations window from the status bar.
impl Explorer {
    pub(super) fn open_operations_window(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if let Some(handle) = self.operations_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return Ok(());
        }
        let bounds = Bounds::centered(None, size(px(780.), px(460.)), cx);
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(460.), px(280.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(rust_explorer::i18n::text("系统操作详情").into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(OperationsWindow::new),
        )?;
        self.operations_window = Some(handle);
        Ok(())
    }
}
