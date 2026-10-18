//! Small native tooltip window used above tab titles. GPUI tooltips are
//! rendered inside the main window, so a tab tooltip anchored at the very top
//! would overlap the tab strip and block the next tab. This popup is a separate
//! non-activating window placed just above the main window instead.
use std::sync::Once;
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
            DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE,
            FW_NORMAL, FillRect, GetDC, GetTextExtentPoint32W, HGDIOBJ, InvalidateRect,
            OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
            TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
            GetClientRect, GetSystemMetrics, GetWindowLongPtrW, HTTRANSPARENT, HWND_TOPMOST,
            MA_NOACTIVATE, RegisterClassW, SM_CXSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
            SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
            WINDOW_STYLE, WM_DESTROY, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT,
            WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
    core::w,
};

const TIP_FONT_SIZE: f32 = 13.;
const TIP_PAD_X: f32 = 10.;
const TIP_PAD_Y: f32 = 6.;

struct TipState {
    text: Vec<u16>,
    dark: bool,
    font: isize,
}

pub struct TabTip {
    hwnd: isize,
    dark: bool,
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
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => unsafe {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TipState;
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let (background, foreground) = if pointer.is_null() {
                (0x202020, 0xffffff)
            } else if (*pointer).dark {
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
                    left: (TIP_PAD_X * 1.4) as i32,
                    top: 0,
                    right: rect.right - (TIP_PAD_X * 1.4) as i32,
                    bottom: rect.bottom,
                };
                let mut text = state.text.clone();
                DrawTextW(
                    dc,
                    &mut text,
                    &mut text_rect,
                    windows::Win32::Graphics::Gdi::DT_LEFT
                        | windows::Win32::Graphics::Gdi::DT_VCENTER
                        | windows::Win32::Graphics::Gdi::DT_SINGLELINE
                        | windows::Win32::Graphics::Gdi::DT_NOPREFIX,
                );
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
            lpszClassName: w!("RustExplorer.TabTip"),
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

impl TabTip {
    pub fn new(scale: f32, dark: bool) -> Option<Self> {
        register_class();
        unsafe {
            let module = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0),
                w!("RustExplorer.TabTip"),
                w!(""),
                WINDOW_STYLE(WS_POPUP.0),
                0,
                0,
                1,
                1,
                None,
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
                dark,
            })
        }
    }

    pub fn show(&mut self, text: &str, x: i32, tab_top: i32, scale: f32) {
        if self.hwnd == 0 {
            return;
        }
        unsafe {
            let hwnd = HWND(self.hwnd as _);
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TipState;
            if pointer.is_null() {
                return;
            }
            let state = &mut *pointer;
            state.text = wide(text);
            state.dark = self.dark;
            let dc = GetDC(Some(hwnd));
            let old = SelectObject(dc, HGDIOBJ(state.font as _));
            let mut size = windows::Win32::Foundation::SIZE::default();
            let body = &state.text[..state.text.len().saturating_sub(1)];
            let _ = GetTextExtentPoint32W(dc, body, &mut size);
            let _ = SelectObject(dc, old);
            let _ = ReleaseDC(Some(hwnd), dc);
            let width = (size.cx + (TIP_PAD_X * 2. * scale) as i32)
                .clamp((80. * scale) as i32, (900. * scale) as i32);
            let height = (size.cy + (TIP_PAD_Y * 2. * scale) as i32).max((26. * scale) as i32);
            let top = tab_top - height - (4. * scale) as i32;
            if top < 0 {
                // No room above the tab. Do not cover the tab / next tab.
                let _ = ShowWindow(hwnd, SW_HIDE);
                return;
            }
            let screen = GetSystemMetrics(SM_CXSCREEN);
            let left = x.min((screen - width - 4).max(4));
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                left,
                top,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(hwnd), None, false);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&self) {
        if self.hwnd != 0 {
            unsafe {
                let _ = ShowWindow(HWND(self.hwnd as _), SW_HIDE);
            }
        }
    }
}

impl Drop for TabTip {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe {
                let _ = DestroyWindow(HWND(self.hwnd as _));
            }
            self.hwnd = 0;
        }
    }
}
