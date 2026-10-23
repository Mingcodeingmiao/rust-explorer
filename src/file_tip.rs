//! Native, click-through hint window for the file hover card.
//!
//! GPUI tooltips are painted inside the main window with their top-left corner at
//! the pointer, and their repaint can land between a right-button down and up
//! (file rows open the context menu on mouse-up). A separate `WS_EX_TRANSPARENT`
//! window keeps the card clear of the pointer — Explorer shows the same
//! information just below-right of it — and can never take a click.
use std::sync::Once;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
            DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE,
            FW_NORMAL, FillRect, GetDC, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
            ReleaseDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GA_ROOT,
            GWLP_USERDATA, GetAncestor, GetClientRect, GetForegroundWindow, GetSystemMetrics,
            GetWindowLongPtrW, GetWindowRect, HTTRANSPARENT, MA_NOACTIVATE, RegisterClassW,
            SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
            SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
            WINDOW_STYLE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT,
            WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
    core::w,
};

const TIP_FONT_SIZE: f32 = 13.;
const TIP_PAD_X: f32 = 12.;
const TIP_PAD_Y: f32 = 8.;
const TIP_MAX_WIDTH: f32 = 440.;
/// Gap between the pointer hotspot and the card. Kept small so the card stays next
/// to the row it describes instead of drifting across the window.
const POINTER_GAP_X: i32 = 10;
const POINTER_GAP_Y: i32 = 16;

const DT_LEFT_TOP: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT =
    windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
        windows::Win32::Graphics::Gdi::DT_LEFT.0
            | windows::Win32::Graphics::Gdi::DT_TOP.0
            | windows::Win32::Graphics::Gdi::DT_NOPREFIX.0
            | windows::Win32::Graphics::Gdi::DT_WORDBREAK.0,
    );

/// The window currently on screen, so a context menu can dismiss it without
/// holding the tooltip entity.
static CURRENT: AtomicIsize = AtomicIsize::new(0);

struct TipState {
    text: Vec<u16>,
    dark: bool,
    font: isize,
}

pub struct FileTip {
    hwnd: isize,
    /// Window that owns the card. Being owned keeps it above that window only, so it
    /// can never float over another application.
    owner: isize,
    dark: bool,
    scale: f32,
    shown: String,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn tip_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // Never take part in hit testing or activation: the card must not be able
        // to swallow the click that opens the context menu.
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => unsafe {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TipState;
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let (background, foreground) = if pointer.is_null() || (*pointer).dark {
                (0x202020, 0xffffff)
            } else {
                (0xf8f8f8, 0x1a1a1a)
            };
            let brush = CreateSolidBrush(COLORREF(background));
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            if !pointer.is_null() {
                let state = &mut *pointer;
                let _ = SelectObject(dc, HGDIOBJ(state.font as _));
                let _ = SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(foreground));
                let mut text_rect = RECT {
                    left: (TIP_PAD_X * 0.6) as i32,
                    top: (TIP_PAD_Y * 0.6) as i32,
                    right: rect.right - (TIP_PAD_X * 0.6) as i32,
                    bottom: rect.bottom - (TIP_PAD_Y * 0.6) as i32,
                };
                let mut text = state.text.clone();
                DrawTextW(dc, &mut text, &mut text_rect, DT_LEFT_TOP);
            }
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        },
        WM_DESTROY => unsafe {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TipState;
            if !pointer.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let state = Box::from_raw(pointer);
                let _ = DeleteObject(HGDIOBJ(state.font as _));
            }
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(tip_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("RustExplorer.FileTip"),
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

impl FileTip {
    /// `owner` is the application window the card belongs to. An owned popup stays
    /// above that window and below every other application, which is what keeps the
    /// card from appearing on top of unrelated windows.
    pub fn new(owner: Option<isize>, scale: f32, dark: bool) -> Option<Self> {
        register_class();
        unsafe {
            let module = GetModuleHandleW(None).ok()?;
            let owner_hwnd = owner
                .filter(|value| *value != 0)
                .map(|value| HWND(value as _));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0),
                w!("RustExplorer.FileTip"),
                w!(""),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                1,
                1,
                owner_hwnd,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .ok()?;
            let font = CreateFontW(
                -(TIP_FONT_SIZE * scale).round() as i32,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                w!("Microsoft YaHei UI"),
            );
            let state = Box::new(TipState {
                text: Vec::new(),
                dark,
                font: font.0 as isize,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            Some(Self {
                hwnd: hwnd.0 as isize,
                owner: owner_hwnd.map(|hwnd| hwnd.0 as isize).unwrap_or(0),
                dark,
                scale,
                shown: String::new(),
            })
        }
    }

    /// Show the card next to the pointer (`cursor` in screen coordinates).
    ///
    /// The card is clamped to the owning window, so it always describes something
    /// inside this window instead of drifting over the desktop or another
    /// application. Re-showing the same text keeps the current position so the card
    /// does not chase the mouse.
    pub fn show(&mut self, text: &str, cursor: (i32, i32)) {
        if self.hwnd == 0 || text.is_empty() {
            return;
        }
        if !self.app_is_foreground() {
            self.hide();
            return;
        }
        if self.shown == text {
            return;
        }
        self.shown = text.to_string();
        unsafe {
            let hwnd = HWND(self.hwnd as _);
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TipState;
            if pointer.is_null() {
                return;
            }
            let state = &mut *pointer;
            state.text = wide(text);
            state.dark = self.dark;

            let padding_x = (TIP_PAD_X * self.scale).round() as i32;
            let padding_y = (TIP_PAD_Y * self.scale).round() as i32;
            let max_width = (TIP_MAX_WIDTH * self.scale).round() as i32;
            let dc = GetDC(Some(hwnd));
            let old = SelectObject(dc, HGDIOBJ(state.font as _));
            let mut measured = RECT {
                left: 0,
                top: 0,
                right: max_width - padding_x * 2,
                bottom: 0,
            };
            let mut text = state.text.clone();
            DrawTextW(
                dc,
                &mut text,
                &mut measured,
                DT_LEFT_TOP | windows::Win32::Graphics::Gdi::DT_CALCRECT,
            );
            let _ = SelectObject(dc, old);
            let _ = ReleaseDC(Some(hwnd), dc);

            let width = (measured.right + padding_x * 2)
                .min(max_width)
                .max((140. * self.scale) as i32);
            let height = (measured.bottom + padding_y * 2).max((30. * self.scale) as i32);

            // Stay inside the owning window: the card describes a row of this window,
            // so it must never drift over the desktop or another application.
            let owner = if self.owner != 0 {
                let mut rect = RECT::default();
                if GetWindowRect(HWND(self.owner as _), &mut rect).is_ok() {
                    Some(rect)
                } else {
                    None
                }
            } else {
                None
            };
            let (min_left, min_top, max_right, max_bottom) = match owner {
                Some(rect) => (rect.left, rect.top, rect.right, rect.bottom),
                None => (
                    0,
                    0,
                    GetSystemMetrics(SM_CXSCREEN),
                    GetSystemMetrics(SM_CYSCREEN),
                ),
            };
            // The pointer left the window while the card was pending: drop it.
            if cursor.0 < min_left
                || cursor.0 > max_right
                || cursor.1 < min_top
                || cursor.1 > max_bottom
            {
                self.hide();
                return;
            }
            let mut left = cursor.0 + POINTER_GAP_X;
            if left + width > max_right {
                left = cursor.0 - width - POINTER_GAP_X;
            }
            let mut top = cursor.1 + POINTER_GAP_Y;
            if top + height > max_bottom {
                // No room below: sit just above the pointer instead.
                top = cursor.1 - height - (POINTER_GAP_Y / 2);
            }
            let left = left.clamp(min_left, (max_right - width).max(min_left));
            let top = top.clamp(min_top, (max_bottom - height).max(min_top));

            // No HWND_TOPMOST: the card is an owned popup, so it sits above its owner
            // and below other applications.
            let _ = SetWindowPos(
                hwnd,
                None,
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            CURRENT.store(self.hwnd, Ordering::Relaxed);
        }
    }

    /// True when this application's window is the foreground window (or owns it).
    fn app_is_foreground(&self) -> bool {
        if self.owner == 0 {
            return true;
        }
        unsafe {
            let foreground = GetForegroundWindow();
            if foreground.0.is_null() {
                return false;
            }
            let root = GetAncestor(foreground, GA_ROOT);
            let root = if root.0.is_null() { foreground } else { root };
            root.0 as isize == self.owner
        }
    }

    pub fn hide(&mut self) {
        self.shown.clear();
        if self.hwnd != 0 {
            unsafe {
                let _ = ShowWindow(HWND(self.hwnd as _), SW_HIDE);
            }
            if CURRENT.load(Ordering::Relaxed) == self.hwnd {
                CURRENT.store(0, Ordering::Relaxed);
            }
        }
    }
}

/// Dismiss the card that is currently on screen (used when a native context menu
/// opens, so the two never overlap).
pub fn hide_current() {
    let hwnd = CURRENT.swap(0, Ordering::Relaxed);
    if hwnd != 0 {
        unsafe {
            let _ = ShowWindow(HWND(hwnd as _), SW_HIDE);
        }
    }
}

impl Drop for FileTip {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            if CURRENT.load(Ordering::Relaxed) == self.hwnd {
                CURRENT.store(0, Ordering::Relaxed);
            }
            unsafe {
                let _ = DestroyWindow(HWND(self.hwnd as _));
            }
            self.hwnd = 0;
        }
    }
}
