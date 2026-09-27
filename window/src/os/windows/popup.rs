//! A popup window (`WindowOps::show_popup`), as Chrome's menus on Windows
//! (MenuHost, a Widget of TYPE_MENU): a WS_POPUP window the application's
//! window owns, a tool window above the others that is never activated
//! (widget_hwnd_utils.cc); its picture, with an alpha of its own, given
//! to the system as a layered window's (LayeredWindowUpdaterImpl::Draw);
//! shown without being activated (ShowInactive), the mouse captured for
//! it while it shows (MenuHost::ShowMenuHost), so that a press beside it
//! is heard of; such a press closes it and is given to the window of this
//! thread's under it (MenuController::RepostEventAndCancel). The keys
//! stay the application's window's.

use super::wide_string;
use crate::os::place_popup;
use crate::{MousePress, Point, PopupEvent, PopupImage};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use winapi::shared::minwindef::*;
use winapi::shared::windef::*;
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::wingdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, RGB,
};
use winapi::um::winuser::*;

const CLASS_NAME: &str = "org.wezfurlong.wezterm.popup";

struct Popup {
    hwnd: HWND,
    owner: HWND,
    /// The picture's part that is the popup itself (its shadow around it)
    body: (usize, usize, usize, usize),
    events: Box<dyn FnMut(PopupEvent) + Send>,
    /// The pointer was over its body.
    inside: bool,
}

thread_local! {
    /// The one popup shown, of the windows of this (the main) thread
    static POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
}

/// Takes the popup shown when it is `hwnd`, or when it is `owner`'s.
fn take(hwnd: Option<HWND>, owner: Option<HWND>) -> Option<Popup> {
    POPUP.with(|popup| {
        let mut popup = popup.borrow_mut();
        let found = popup.as_ref().is_some_and(|p| {
            hwnd.map_or(true, |h| h == p.hwnd) && owner.map_or(true, |o| o == p.owner)
        });
        if found {
            popup.take()
        } else {
            None
        }
    })
}

/// Tells the handler of the popup `hwnd`; its state is free meanwhile
/// (what the handler does may come back to this window's procedure).
fn send(hwnd: HWND, event: PopupEvent) {
    let events = POPUP.with(|popup| {
        popup
            .borrow_mut()
            .as_mut()
            .filter(|p| p.hwnd == hwnd)
            .map(|p| std::mem::replace(&mut p.events, Box::new(|_| {})))
    });
    let Some(mut events) = events else {
        return;
    };
    events(event);
    POPUP.with(|popup| {
        if let Some(p) = popup.borrow_mut().as_mut().filter(|p| p.hwnd == hwnd) {
            p.events = events;
        }
    });
}

unsafe fn destroy(popup: &Popup) {
    if GetCapture() == popup.hwnd {
        ReleaseCapture();
    }
    DestroyWindow(popup.hwnd);
}

/// The popup `hwnd` goes, not by `close`: its handler hears of it.
unsafe fn dismiss(hwnd: HWND) {
    if let Some(mut popup) = take(Some(hwnd), None) {
        (popup.events)(PopupEvent::Dismissed);
        destroy(&popup);
    }
}

/// The press `msg` at the screen's point, which closed the popup of
/// `owner`, to the window under it (Chrome's RepostEventImpl): posted to
/// that window when it is one of the owner's thread (another thread's
/// hears of the press from the system, capture or not), for its client
/// area as it came, for the rest of it as the press there that it is.
unsafe fn repost(owner: HWND, msg: UINT, wparam: WPARAM, screen: POINT) {
    let target = WindowFromPoint(screen);
    if target.is_null()
        || owner.is_null()
        || GetWindowThreadProcessId(target, null_mut())
            != GetWindowThreadProcessId(owner, null_mut())
    {
        return;
    }
    let coords = |x: i32, y: i32| ((y as u16 as u32) << 16 | (x as u16 as u32)) as LPARAM;
    let hit = SendMessageW(target, WM_NCHITTEST, 0, coords(screen.x, screen.y));
    if hit == HTCLIENT as LRESULT {
        let mut client = screen;
        ScreenToClient(target, &mut client);
        PostMessageW(target, msg, wparam, coords(client.x, client.y));
        return;
    }
    let msg = match msg {
        WM_LBUTTONDOWN => WM_NCLBUTTONDOWN,
        WM_RBUTTONDOWN => WM_NCRBUTTONDOWN,
        WM_MBUTTONDOWN => WM_NCMBUTTONDOWN,
        WM_LBUTTONDBLCLK => WM_NCLBUTTONDBLCLK,
        WM_RBUTTONDBLCLK => WM_NCRBUTTONDBLCLK,
        WM_MBUTTONDBLCLK => WM_NCMBUTTONDBLCLK,
        _ => return,
    };
    PostMessageW(target, msg, hit as WPARAM, coords(screen.x, screen.y));
}

/// The picture, to the system (Chrome's LayeredWindowUpdaterImpl::Draw:
/// UpdateLayeredWindow from a bitmap with a premultiplied alpha, which
/// is what `image` is). The window takes the picture's size.
unsafe fn paint(hwnd: HWND, image: &PopupImage) {
    let (width, height) = (image.width as i32, image.height as i32);
    let bytes = image.width * image.height * 4;
    if width <= 0 || height <= 0 || image.data.len() < bytes {
        return;
    }
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    // the first row is the top one
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let dc = CreateCompatibleDC(null_mut());
    if dc.is_null() {
        return;
    }
    let mut bits = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if !bitmap.is_null() && !bits.is_null() {
        std::ptr::copy_nonoverlapping(image.data.as_ptr(), bits as *mut u8, bytes);
        let before = SelectObject(dc, bitmap as _);
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);
        let mut position = POINT {
            x: rect.left,
            y: rect.top,
        };
        let mut size = SIZE {
            cx: width,
            cy: height,
        };
        let mut zero = POINT { x: 0, y: 0 };
        let mut blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER,
            BlendFlags: 0,
            SourceConstantAlpha: 0xff,
            AlphaFormat: AC_SRC_ALPHA,
        };
        if UpdateLayeredWindow(
            hwnd,
            null_mut(),
            &mut position,
            &mut size,
            dc,
            &mut zero,
            RGB(0xff, 0xff, 0xff),
            &mut blend,
            ULW_ALPHA,
        ) == 0
        {
            log::error!(
                "popup: UpdateLayeredWindow: {}",
                std::io::Error::last_os_error()
            );
        }
        SelectObject(dc, before);
    }
    if !bitmap.is_null() {
        DeleteObject(bitmap as _);
    }
    DeleteDC(dc);
}

/// `WindowOps::show_popup` for the window `owner`. Not to be called with
/// a window's state borrowed: the system calls the windows' procedures
/// from here.
pub(crate) fn show(
    owner: HWND,
    at: Point,
    image: PopupImage,
    mut events: Box<dyn FnMut(PopupEvent) + Send>,
) {
    unsafe {
        // one at a time; another window's hears that it went
        if let Some(mut shown) = take(None, None) {
            if shown.owner != owner {
                (shown.events)(PopupEvent::Dismissed);
            }
            destroy(&shown);
        }

        let mut origin = POINT {
            x: at.x as i32,
            y: at.y as i32,
        };
        ClientToScreen(owner, &mut origin);
        // within the work area of the point's monitor (MenuController:
        // the display nearest the anchor, its work_area)
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(
            MonitorFromPoint(origin, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        );
        let work = monitor.rcWork;
        let body = image.body;
        let (x, y) = place_popup(
            (origin.x, origin.y),
            (body.2 as i32, body.3 as i32),
            (
                work.left,
                work.top,
                work.right - work.left,
                work.bottom - work.top,
            ),
        );
        let (x, y) = (x - body.0 as i32, y - body.1 as i32);

        let class_name = wide_string(CLASS_NAME);
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
        };
        // (registered already, from the second popup on)
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name.as_ptr(),
            wide_string("").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            x,
            y,
            image.width as i32,
            image.height as i32,
            owner,
            null_mut(),
            instance,
            null_mut(),
        );
        if hwnd.is_null() {
            log::error!(
                "popup: CreateWindowExW: {}",
                std::io::Error::last_os_error()
            );
            events(PopupEvent::Dismissed);
            return;
        }
        POPUP.with(|popup| {
            popup.borrow_mut().replace(Popup {
                hwnd,
                owner,
                body,
                events,
                inside: false,
            })
        });
        paint(hwnd, &image);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetCapture(hwnd);
    }
}

/// `WindowOps::update_popup` for the window `owner`.
pub(crate) fn update(owner: HWND, image: PopupImage) {
    let hwnd = POPUP.with(|popup| {
        popup
            .borrow_mut()
            .as_mut()
            .filter(|p| p.owner == owner)
            .map(|p| {
                p.body = image.body;
                p.hwnd
            })
    });
    if let Some(hwnd) = hwnd {
        unsafe { paint(hwnd, &image) };
    }
}

/// `WindowOps::close_popup` for the window `owner`.
pub(crate) fn close(owner: HWND) {
    if let Some(popup) = take(None, Some(owner)) {
        unsafe { destroy(&popup) };
    }
}

/// Where the pointer is, of the popup's picture (beside it too: the
/// mouse is captured).
fn coords(lparam: LPARAM) -> (isize, isize) {
    (
        (lparam & 0xffff) as u16 as i16 as isize,
        ((lparam >> 16) & 0xffff) as u16 as i16 as isize,
    )
}

/// Whether that is on the popup `hwnd` itself, not on its shadow or
/// beside it.
fn within(hwnd: HWND, (x, y): (isize, isize)) -> bool {
    POPUP.with(|popup| {
        popup
            .borrow()
            .as_ref()
            .filter(|p| p.hwnd == hwnd)
            .is_some_and(|p| {
                let (left, top, width, height) = p.body;
                x >= left as isize
                    && y >= top as isize
                    && x < (left + width) as isize
                    && y < (top + height) as isize
            })
    })
}

/// Sets whether the pointer is over the body, and says whether it was.
fn set_inside(hwnd: HWND, inside: bool) -> bool {
    POPUP.with(|popup| {
        popup
            .borrow_mut()
            .as_mut()
            .filter(|p| p.hwnd == hwnd)
            .is_some_and(|p| std::mem::replace(&mut p.inside, inside))
    })
}

unsafe fn do_wnd_proc(hwnd: HWND, msg: UINT, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let press = match msg {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => Some(MousePress::Left),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_RBUTTONUP => Some(MousePress::Right),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK | WM_MBUTTONUP => Some(MousePress::Middle),
        _ => None,
    };
    match msg {
        WM_MOUSEACTIVATE => Some(MA_NOACTIVATE as LRESULT),
        WM_MOUSEMOVE => {
            let at = coords(lparam);
            if within(hwnd, at) {
                set_inside(hwnd, true);
                // (no WM_SETCURSOR for a window that has the capture)
                SetCursor(LoadCursorW(null_mut(), IDC_ARROW));
                send(hwnd, PopupEvent::Motion(at.0, at.1));
            } else if set_inside(hwnd, false) {
                send(hwnd, PopupEvent::Leave);
            }
            Some(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN
        | WM_MBUTTONDBLCLK => {
            let at = coords(lparam);
            if within(hwnd, at) {
                send(hwnd, PopupEvent::Press(at.0, at.1, press?));
            } else {
                // a press beside it: it goes, and the press is the
                // window's under it (found while the popup is there to
                // say where the press was)
                let mut screen = POINT {
                    x: at.0 as i32,
                    y: at.1 as i32,
                };
                ClientToScreen(hwnd, &mut screen);
                let owner = GetWindow(hwnd, GW_OWNER);
                dismiss(hwnd);
                repost(owner, msg, wparam, screen);
            }
            Some(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let at = coords(lparam);
            if within(hwnd, at) {
                send(hwnd, PopupEvent::Release(at.0, at.1, press?));
            }
            Some(0)
        }
        WM_CAPTURECHANGED => {
            // another window has the mouse: a press on another
            // application, another window brought forward
            if lparam as HWND != hwnd {
                dismiss(hwnd);
            }
            Some(0)
        }
        WM_NCDESTROY => {
            // (destroyed with its owner: nobody is left to hear of it)
            take(Some(hwnd), None);
            None
        }
        _ => None,
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match std::panic::catch_unwind(|| {
        do_wnd_proc(hwnd, msg, wparam, lparam)
            .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam))
    }) {
        Ok(result) => result,
        Err(e) => {
            log::error!("caught {:?}", e);
            std::process::exit(1)
        }
    }
}
