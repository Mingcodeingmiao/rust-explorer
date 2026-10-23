//! Give the native top resize border priority over the transparent titlebar.
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    UI::{
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const SUBCLASS_ID: usize = 0x52584546;

fn top_hit(x: i32, y: i32, width: i32, height: i32, frame_x: i32, frame_y: i32) -> Option<u32> {
    if x < 0 || x >= width || y < 0 || y >= height || y >= frame_y {
        return None;
    }
    Some(if x < frame_x {
        HTTOPLEFT
    } else if x >= width - frame_x {
        HTTOPRIGHT
    } else {
        HTTOP
    })
}

pub fn install(handle: isize) -> anyhow::Result<()> {
    let hwnd = HWND(handle as *mut _);
    anyhow::ensure!(
        unsafe { SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, 0) }.as_bool(),
        "无法安装窗口顶部缩放处理"
    );
    Ok(())
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCDESTROY {
            crate::instance::withdraw(hwnd.0 as isize);
            let _ = RemoveWindowSubclass(hwnd, Some(frame_proc), id);
        } else if msg == WM_COPYDATA {
            return crate::instance::receive(hwnd, lparam);
        } else if matches!(msg, WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK)
            && wparam.0 == HTCAPTION as usize
        {
            // Let Windows own caption dragging/restoring. Dispatching this as
            // a GPUI mouse event can have its default action suppressed by focus
            // handlers, especially when dragging a maximized window down.
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        } else if msg == WM_NCHITTEST
            && !IsZoomed(hwnd).as_bool()
            && !IsIconic(hwnd).as_bool()
            && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 != 0
        {
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_ok() {
                // Signed coordinates are required for monitors left/above the primary one.
                let screen_x = (lparam.0 as u16 as i16) as i32;
                let screen_y = ((lparam.0 >> 16) as u16 as i16) as i32;
                let dpi = GetDpiForWindow(hwnd).max(96);
                let padding = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let frame_x = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + padding;
                let frame_y = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + padding;
                if let Some(hit) = top_hit(
                    screen_x - rect.left,
                    screen_y - rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    frame_x,
                    frame_y,
                ) {
                    return LRESULT(hit as isize);
                }
            }
        }
        // GPUI and Windows retain all other hit tests, sizing and caption actions.
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn top_border_and_corners_scale_without_capturing_tabs() {
        for border in [8, 14, 16] {
            assert_eq!(
                top_hit(100, border - 1, 800, 600, border, border),
                Some(HTTOP)
            );
            assert_eq!(top_hit(0, 0, 800, 600, border, border), Some(HTTOPLEFT));
            assert_eq!(
                top_hit(799, border - 1, 800, 600, border, border),
                Some(HTTOPRIGHT)
            );
            assert_eq!(top_hit(100, border, 800, 600, border, border), None);
            assert_eq!(top_hit(-1, 1, 800, 600, border, border), None);
            assert_eq!(top_hit(100, -1, 800, 600, border, border), None);
            assert_eq!(top_hit(800, 0, 800, 600, border, border), None);
        }
    }
}
