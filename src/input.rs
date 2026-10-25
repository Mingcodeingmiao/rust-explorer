// Adapted from GPUI's Apache-2.0 licensed input example (Zed Industries).
use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, prelude::*,
    px, relative, rgb, size,
};
use unicode_segmentation::*;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
        Quit,
        Submit,
        Cancel,
        SuggestPrevious,
        SuggestNext,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_lines: Vec<WrappedLine>,
    last_line_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Pixels,
    is_selecting: bool,
    pub compact: bool,
    /// Logical wrap width for callers that render in a fixed-width overlay.
    pub wrap_width: Option<f32>,
    /// Hard-wrap display into chunks of this many characters. Content itself
    /// never contains newlines (GPUI's `shape_text` rejects them).
    pub wrap_chars: Option<usize>,
    pub single_line_scroll: bool,
    pub click_to_end: bool,
    pub double_click_select_all: bool,
    drag_anchor: Option<Point<Pixels>>,
    scroll_x: Pixels,
}

impl TextInput {
    pub fn new(value: String, placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: value.into(),
            placeholder: placeholder.to_owned().into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_line_starts: Vec::new(),
            last_bounds: None,
            last_line_height: px(20.),
            is_selecting: false,
            compact: false,
            wrap_width: None,
            wrap_chars: None,
            single_line_scroll: false,
            click_to_end: false,
            double_click_select_all: false,
            drag_anchor: None,
            scroll_x: px(0.),
        }
    }
    fn suggest_previous(&mut self, _: &SuggestPrevious, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Suggestion(-1));
    }
    fn suggest_next(&mut self, _: &SuggestNext, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Suggestion(1));
    }
    pub fn value(&self) -> String {
        self.content.to_string()
    }
    pub fn text_command_available(&self, command: &str, can_paste: bool) -> bool {
        match command {
            "cut" | "copy" => !self.selected_range.is_empty(),
            "paste" => can_paste,
            "all" => !self.content.is_empty(),
            _ => false,
        }
    }
    pub fn text_command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        match command {
            "cut" => self.cut(&Cut, window, cx),
            "copy" => self.copy(&Copy, window, cx),
            "paste" => self.paste(&Paste, window, cx),
            _ => self.select_all(&SelectAll, window, cx),
        }
    }
    pub fn set_value(&mut self, value: String, cx: &mut Context<Self>) {
        self.content = value.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }
    pub fn set_placeholder(&mut self, placeholder: String, cx: &mut Context<Self>) {
        if self.placeholder.as_ref() == placeholder {
            return;
        }
        self.placeholder = placeholder.into();
        cx.notify();
    }
    pub fn focus_select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.select_all(&SelectAll, window, cx);
    }
    pub fn focus_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.move_to(self.content.len(), cx);
    }
    pub fn focus_filename(&mut self, window: &mut Window, cx: &mut Context<Self>, directory: bool) {
        self.focus_select_all(window, cx);
        if !directory {
            if let Some(dot) = self.content.rfind('.') {
                if dot > 0 {
                    self.selected_range = 0..dot;
                }
            }
        }
        cx.notify();
    }
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        cx.stop_propagation();
        self.is_selecting = true;
        self.drag_anchor = None;

        if self.double_click_select_all && event.click_count >= 2 {
            self.is_selecting = false;
            self.select_all(&SelectAll, window, cx);
            return;
        }

        if self.click_to_end && !event.modifiers.shift {
            self.drag_anchor = Some(event.position);
            self.move_to(self.content.len(), cx);
            return;
        }

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_anchor = None;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            if let Some(anchor) = self.drag_anchor {
                let delta = event.position - anchor;
                if delta.x.abs() < px(3.) && delta.y.abs() < px(3.) {
                    return;
                }
                self.move_to(self.index_for_mouse_position(anchor), cx);
                self.drag_anchor = None;
            }
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let lines = &self.last_lines;
        let starts = &self.last_line_starts;
        let line_index = starts
            .iter()
            .rposition(|start| *start <= offset)
            .unwrap_or(0)
            .min(lines.len().checked_sub(1)?);
        let line = &lines[line_index];
        let local = offset.saturating_sub(starts[line_index]);
        line.position_for_index(local.min(line.text.len()), self.last_line_height)
            .map(|position| {
                position + gpui::point(px(0.), self.last_line_height * line_index as f32)
            })
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let lines = &self.last_lines;
        if lines.is_empty() {
            return 0;
        }
        let local_y = position.y - bounds.top();
        let line_index = ((local_y / self.last_line_height).floor() as usize).min(lines.len() - 1);
        let start = self.last_line_starts[line_index];
        let line = &lines[line_index];
        let local_x = position.x - bounds.left() + self.scroll_x;
        if local_x <= px(0.) || line.text.is_empty() {
            return start;
        }
        let end_x = line
            .position_for_index(line.text.len(), self.last_line_height)
            .map(|position| position.x)
            .unwrap_or(px(0.));
        if local_x >= end_x {
            return start + line.text.len();
        }
        start
            + line
                .closest_index_for_position(gpui::point(local_x, px(0.)), self.last_line_height)
                .unwrap_or(0)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selection| {
                let offset = |target: usize| {
                    let (mut units, mut bytes) = (0, 0);
                    for ch in new_text.chars() {
                        if units >= target {
                            break;
                        }
                        units += ch.len_utf16();
                        bytes += ch.len_utf8();
                    }
                    bytes
                };
                range.start + offset(selection.start)..range.start + offset(selection.end)
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        Some(Bounds::new(
            bounds.origin + start - gpui::point(self.scroll_x, px(0.)),
            size(
                if start.y == end.y {
                    (end.x - start.x).max(px(1.))
                } else {
                    px(1.)
                },
                self.last_line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        let input = self.input.read(cx);
        style.size.width = relative(1.).into();
        style.size.height =
            if input.compact || input.wrap_width.is_some() || input.wrap_chars.is_some() {
                relative(1.).into()
            } else {
                window.line_height().into()
            };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), rgb(0x888888).into())
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        // Split existing IME runs at selection boundaries so selected glyphs
        // remain white in both themes without losing composition underlines.
        if !selected_range.is_empty() {
            let mut offset = 0;
            runs = runs
                .into_iter()
                .flat_map(|run| {
                    let start = offset;
                    offset += run.len;
                    let mut boundaries = vec![start, offset];
                    for boundary in [selected_range.start, selected_range.end] {
                        if boundary > start && boundary < offset {
                            boundaries.push(boundary);
                        }
                    }
                    boundaries.sort_unstable();
                    boundaries
                        .windows(2)
                        .map(|pair| TextRun {
                            len: pair[1] - pair[0],
                            color: if selected_range.contains(&pair[0]) {
                                rgb(0xffffff).into()
                            } else {
                                run.color
                            },
                            ..run.clone()
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
        }

        let font_size = style.font_size.to_pixels(window.rem_size());
        let display_source = display_text.clone();
        // GPUI's `shape_text` panics on newlines. For hard-wrapped editors we
        // shape each character chunk separately and never put a newline into
        // the input value.
        let lines = if let Some(chunk_chars) = input.wrap_chars {
            let chars: Vec<char> = display_source.chars().collect();
            let mut chunks: Vec<String> = chars
                .chunks(chunk_chars.max(1))
                .map(|chunk| chunk.iter().collect())
                .collect();
            if chunks.is_empty() {
                chunks.push(String::new());
            }
            chunks
                .into_iter()
                .map(|chunk| {
                    let chunk_run = TextRun {
                        len: chunk.len(),
                        font: style.font(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    window
                        .text_system()
                        .shape_text(
                            SharedString::from(chunk),
                            font_size,
                            &[chunk_run],
                            None,
                            None,
                        )
                        .unwrap()
                        .remove(0)
                })
                .collect::<Vec<_>>()
        } else {
            window
                .text_system()
                .shape_text(
                    display_text,
                    font_size,
                    &runs,
                    input
                        .wrap_width
                        .map(|width| px(width * window.scale_factor()))
                        .or_else(|| input.compact.then_some(bounds.size.width)),
                    None,
                )
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let line_height = window.line_height();
        let source = display_source.as_bytes();
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut start = 0usize;
        for line in &lines {
            line_starts.push(start);
            start += line.text.len();
            // `shape_text` splits at the newline and excludes it from `text`;
            // advance past it so cursor offsets stay global.
            while matches!(source.get(start), Some(b'\r' | b'\n')) {
                start += 1;
            }
        }
        let cursor_line = line_starts
            .iter()
            .rposition(|start| *start <= cursor)
            .unwrap_or(0);
        let cursor_local = cursor.saturating_sub(line_starts[cursor_line]);
        let cursor_pos = lines[cursor_line]
            .position_for_index(cursor_local, line_height)
            .unwrap_or_default()
            + gpui::point(px(0.), line_height * cursor_line as f32);
        let scroll_x = if input.single_line_scroll {
            let right = (bounds.size.width - px(2.)).max(px(1.));
            let mut offset = input.scroll_x;
            if cursor_pos.x < offset {
                offset = cursor_pos.x;
            }
            if cursor_pos.x > offset + right {
                offset = cursor_pos.x - right;
            }
            let end = lines[0]
                .position_for_index(lines[0].len(), line_height)
                .unwrap_or_default()
                .x;
            offset.clamp(px(0.), (end - right).max(px(0.)))
        } else {
            px(0.)
        };
        let origin = bounds.origin - gpui::point(scroll_x, px(0.));
        let mut selection = Vec::new();
        if !selected_range.is_empty() {
            for (line_index, line) in lines.iter().enumerate() {
                let line_start = line_starts[line_index];
                let line_end = line_start + line.text.len();
                if selected_range.end <= line_start || selected_range.start >= line_end {
                    continue;
                }
                for (start, ch) in line.text.char_indices() {
                    let global = line_start + start;
                    if !selected_range.contains(&global) {
                        continue;
                    }
                    let p = line
                        .position_for_index(start, line_height)
                        .unwrap_or_default()
                        + gpui::point(px(0.), line_height * line_index as f32);
                    let q = line
                        .position_for_index(start + ch.len_utf8(), line_height)
                        .unwrap_or(p - gpui::point(px(0.), line_height * line_index as f32))
                        + gpui::point(px(0.), line_height * line_index as f32);
                    let width = if q.y == p.y {
                        q.x - p.x
                    } else {
                        bounds.size.width - p.x
                    };
                    selection.push(fill(
                        Bounds::new(origin + p, size(width.max(px(1.)), line_height)),
                        // Windows text selection: rgb(0,120,215).
                        rgb(0x0078D7),
                    ));
                }
            }
        }
        let cursor = selected_range.is_empty().then(|| {
            fill(
                Bounds::new(origin + cursor_pos, size(px(2.), line_height)),
                style.color,
            )
        });
        PrepaintState {
            lines,
            line_starts,
            cursor,
            selection,
            scroll_x,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection)
        }
        for (index, line) in prepaint.lines.iter().enumerate() {
            line.paint(
                bounds.origin - gpui::point(prepaint.scroll_x, px(0.))
                    + gpui::point(px(0.), window.line_height() * index as f32),
                window.line_height(),
                gpui::TextAlign::Left,
                Some(bounds),
                window,
                cx,
            )
            .unwrap();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_line_height = window.line_height();
            input.last_lines = prepaint.lines.clone();
            input.last_line_starts = prepaint.line_starts.clone();
            input.last_bounds = Some(bounds);
            input.scroll_x = prepaint.scroll_x;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::suggest_previous))
            .on_action(cx.listener(Self::suggest_next))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, _, w, cx| {
                    cx.stop_propagation();
                    w.focus(&v.focus_handle);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, e: &MouseUpEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(InputEvent::ContextMenu(e.position));
                }),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .when(
                self.compact || self.wrap_width.is_some() || self.wrap_chars.is_some(),
                |d| d.h_full(),
            )
            .overflow_hidden()
            .line_height(gpui::rems(if self.compact { 1.125 } else { 1.375 }))
            .text_size(gpui::rems(0.8125))
            .child(
                div()
                    .when(
                        self.compact || self.wrap_width.is_some() || self.wrap_chars.is_some(),
                        |d| d.h_full(),
                    )
                    .when(!self.compact, |d| d.h(gpui::rems(1.875)))
                    .w_full()
                    .p(gpui::rems(if self.compact { 0.0625 } else { 0.25 }))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub enum InputEvent {
    ContextMenu(Point<Pixels>),
    Suggestion(i32),
    Changed,
    Submit,
    Cancel,
}
impl gpui::EventEmitter<InputEvent> for TextInput {}
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SuggestPrevious, Some("TextInput")),
        KeyBinding::new("down", SuggestNext, Some("TextInput")),
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("enter", Submit, Some("TextInput")),
        KeyBinding::new("escape", Cancel, Some("TextInput")),
    ]);
}
