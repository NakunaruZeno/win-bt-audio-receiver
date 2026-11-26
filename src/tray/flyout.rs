use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWCP_ROUND, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect, GetDeviceCaps,
    GetMonitorInfoW, InvalidateRect, LineTo, MonitorFromPoint, MoveToEx, Polyline, RoundRect,
    SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, HPEN, LOGPIXELSX,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, PS_NULL, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DrawIconEx, GetClientRect, GetCursorPos,
    LoadCursorW, RegisterClassW, SetCursor, SetForegroundWindow, ShowWindow, DI_NORMAL, HICON,
    HTCLIENT, IDC_ARROW, IDC_HAND, SW_SHOW, WA_INACTIVE, WM_ACTIVATE, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::bluetooth::PairedDevice;
use crate::config::{load_saved_bt_device, TransportMode};
use crate::tray::{
    is_autostart_enabled, is_debug_log_visible, mode_display_name, open_config_folder,
    select_bt_device, set_autostart_enabled, spawn_bt_device_scan, toggle_debug_log_window,
    toggle_pause, tray_state, trigger_mode_switch, TraySharedState,
};

pub const FLYOUT_WND_CLASS_NAME: PCWSTR = w!("AetherSoraFlyoutWndClass");

const LOGICAL_WIDTH: i32 = 328;
const LOGICAL_HEIGHT: i32 = 364;
const EDGE_MARGIN: i32 = 8;
const MAX_BT_VISIBLE_ROWS: usize = 5;

const MODES: [TransportMode; 4] = [
    TransportMode::Auto,
    TransportMode::WifiApp,
    TransportMode::WifiAdb,
    TransportMode::Bluetooth,
];

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

const BG_DEEP_SKY: COLORREF = rgb(0x0B, 0x13, 0x2B);
const BG_HEADER: COLORREF = rgb(0x10, 0x1D, 0x42);
const BG_SURFACE: COLORREF = rgb(0x16, 0x25, 0x50);
const BG_SURFACE_HOVER: COLORREF = rgb(0x1E, 0x33, 0x6D);
const BG_DROPDOWN: COLORREF = rgb(0x0E, 0x19, 0x38);
const BG_DROPDOWN_HOVER: COLORREF = rgb(0x1C, 0x30, 0x64);
const BG_DISABLED: COLORREF = rgb(0x12, 0x1D, 0x3F);

const BORDER_SUBTLE: COLORREF = rgb(0x25, 0x3C, 0x7A);
const DIVIDER_LINE: COLORREF = rgb(0x1D, 0x2F, 0x63);

const ACCENT_SKY: COLORREF = rgb(0x38, 0xBD, 0xF8);
const ACCENT_GOLD: COLORREF = rgb(0xFA, 0xCC, 0x15);
const ACCENT_GOLD_HOVER: COLORREF = rgb(0xFD, 0xE0, 0x47);
const STATUS_CONNECTED: COLORREF = rgb(0x34, 0xD3, 0x99);

const TEXT_PRIMARY: COLORREF = rgb(0xF8, 0xFA, 0xFC);
const TEXT_MUTED: COLORREF = rgb(0x94, 0xA3, 0xB8);
const TEXT_DARK: COLORREF = rgb(0x0B, 0x13, 0x2B);

const DANGER_BG_HOVER: COLORREF = rgb(0x4C, 0x1D, 0x31);
const DANGER_BORDER_HOVER: COLORREF = rgb(0xFB, 0x71, 0x85);
const DANGER_TEXT_HOVER: COLORREF = rgb(0xFE, 0xCD, 0xD3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenDropdown {
    None,
    Mode,
    BtDevice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitTarget {
    None,
    PauseButton,
    ModeBox,
    BtScanButton,
    BtBox,
    AutostartRow,
    DebugLogRow,
    OpenConfigBtn,
    ExitBtn,
    ModeOption(usize),
    BtOption(usize),
    BtScanRow,
}

struct FlyoutUi {
    dpi: i32,
    open_dropdown: OpenDropdown,
    bt_scroll_offset: usize,
    hover: HitTarget,
    tracking_mouse: bool,
    font_title: HFONT,
    font_body: HFONT,
    font_semi: HFONT,
    font_label: HFONT,
}

impl FlyoutUi {
    unsafe fn new(dpi: i32) -> Self {
        Self {
            dpi,
            open_dropdown: OpenDropdown::None,
            bt_scroll_offset: 0,
            hover: HitTarget::None,
            tracking_mouse: false,
            font_title: make_ui_font(16, 600, dpi),
            font_body: make_ui_font(13, 400, dpi),
            font_semi: make_ui_font(13, 600, dpi),
            font_label: make_ui_font(11, 600, dpi),
        }
    }

    unsafe fn free_fonts(&mut self) {
        for f in [
            &mut self.font_title,
            &mut self.font_body,
            &mut self.font_semi,
            &mut self.font_label,
        ] {
            if !f.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(f.0));
                *f = HFONT::default();
            }
        }
    }
}

thread_local! {
    static ACTIVE_UI: RefCell<Option<FlyoutUi>> = const { RefCell::new(None) };
    static CLASS_REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

#[inline]
fn s(logical: i32, dpi: i32) -> i32 {
    (logical * dpi + 48) / 96
}

fn pt_in(rc: &RECT, x: i32, y: i32) -> bool {
    x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom
}

/// Toggles the Celestial Sky flyout near the tray cursor (or destroys it if open).
pub(crate) unsafe fn toggle_flyout(shared: &Arc<TraySharedState>, anchor_to_cursor: bool) {
    let existing = shared.flyout_hwnd_raw.load(Ordering::Acquire);
    if existing != 0 {
        let _ = DestroyWindow(HWND(existing as *mut core::ffi::c_void));
        return;
    }

    if let Ok(closed_at) = shared.last_flyout_closed.lock() {
        if closed_at.is_some_and(|t| t.elapsed().as_millis() < 180) {
            return;
        }
    }

    create_and_show_flyout(shared, anchor_to_cursor);
}

/// Opens the Celestial Sky flyout anchored cleanly inside the primary work area bottom-right.
pub(crate) unsafe fn open_flyout_anchored(shared: &Arc<TraySharedState>) {
    let existing = shared.flyout_hwnd_raw.load(Ordering::Acquire);
    if existing != 0 {
        return;
    }
    create_and_show_flyout(shared, false);
}

pub(crate) fn request_repaint(shared: &Arc<TraySharedState>) {
    let raw = shared.flyout_hwnd_raw.load(Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let _ = InvalidateRect(HWND(raw as *mut core::ffi::c_void), None, false);
        }
    }
}

pub(crate) unsafe fn destroy_if_open(shared: &Arc<TraySharedState>) {
    let raw = shared.flyout_hwnd_raw.load(Ordering::Acquire);
    if raw != 0 {
        let _ = DestroyWindow(HWND(raw as *mut core::ffi::c_void));
    }
}

unsafe fn create_and_show_flyout(shared: &Arc<TraySharedState>, anchor_to_cursor: bool) {
    let Ok(hmodule) = GetModuleHandleW(None) else {
        return;
    };
    let hinstance = HINSTANCE(hmodule.0);

    CLASS_REGISTERED.with(|reg| {
        if !*reg.borrow() {
            let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(flyout_wnd_proc),
                hInstance: hinstance,
                hCursor: cursor,
                lpszClassName: FLYOUT_WND_CLASS_NAME,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            *reg.borrow_mut() = true;
        }
    });

    let dpi = query_system_dpi();
    let width = s(LOGICAL_WIDTH, dpi);
    let height = s(LOGICAL_HEIGHT, dpi);
    let (x, y) = compute_flyout_position(width, height, dpi, anchor_to_cursor);

    ACTIVE_UI.with(|cell| {
        if let Some(mut old) = cell.borrow_mut().take() {
            old.free_fonts();
        }
        *cell.borrow_mut() = Some(FlyoutUi::new(dpi));
    });

    let Ok(hwnd) = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        FLYOUT_WND_CLASS_NAME,
        w!("AetherSora Quick Controls"),
        WS_POPUP,
        x,
        y,
        width,
        height,
        None,
        None,
        hinstance,
        None,
    ) else {
        ACTIVE_UI.with(|cell| {
            if let Some(mut old) = cell.borrow_mut().take() {
                old.free_fonts();
            }
        });
        return;
    };

    let corner_pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        (&corner_pref as *const DWM_WINDOW_CORNER_PREFERENCE).cast::<core::ffi::c_void>(),
        std::mem::size_of_val(&corner_pref) as u32,
    );
    let border_col = BORDER_SUBTLE.0;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_BORDER_COLOR,
        (&border_col as *const u32).cast::<core::ffi::c_void>(),
        std::mem::size_of::<u32>() as u32,
    );

    shared.flyout_hwnd_raw.store(hwnd.0 as isize, Ordering::Release);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
}

unsafe fn query_system_dpi() -> i32 {
    let hdc = windows::Win32::Graphics::Gdi::GetDC(None);
    if hdc.is_invalid() {
        return 96;
    }
    let dpi = GetDeviceCaps(hdc, LOGPIXELSX);
    let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, hdc);
    if dpi > 0 { dpi } else { 96 }
}

unsafe fn compute_flyout_position(
    width: i32,
    height: i32,
    dpi: i32,
    anchor_to_cursor: bool,
) -> (i32, i32) {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let margin = s(EDGE_MARGIN, dpi);

    if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
        return (100, 100);
    }
    let work = mi.rcWork;

    if !anchor_to_cursor {
        return (
            (work.right - width - margin).max(work.left + margin),
            (work.bottom - height - margin).max(work.top + margin),
        );
    }

    let x = (pt.x - width / 2)
        .max(work.left + margin)
        .min(work.right - width - margin);
    let mid_y = (work.top + work.bottom) / 2;
    let raw_y = if pt.y >= mid_y {
        pt.y - height - margin
    } else {
        pt.y + margin
    };
    let y = raw_y
        .max(work.top + margin)
        .min(work.bottom - height - margin);

    (x, y)
}

#[derive(Clone, Debug)]
struct BtMenuData {
    rows: Vec<PairedDevice>,
    saved_id: Option<String>,
    display_label: String,
    scanning: bool,
}

fn collect_bt_menu_data(shared: &TraySharedState) -> BtMenuData {
    let saved = load_saved_bt_device();
    let cached = shared
        .bt_devices
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let scanning = shared.bt_scanning.load(Ordering::Acquire);

    let rows = if !cached.is_empty() {
        cached
    } else if let Some((ref id, ref name)) = saved {
        vec![PairedDevice {
            id: id.clone(),
            name: name.clone(),
            is_enabled: true,
        }]
    } else {
        Vec::new()
    };

    let display_label = saved
        .as_ref()
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| "(Select or Scan device)".to_owned());

    BtMenuData {
        rows,
        saved_id: saved.map(|(id, _)| id),
        display_label,
        scanning,
    }
}

struct FlyoutLayout {
    header: RECT,
    icon_rect: RECT,
    title_rect: RECT,
    version_rect: RECT,
    status_dot_rect: RECT,
    status_text_rect: RECT,
    pause_btn: RECT,
    mode_label: RECT,
    mode_box: RECT,
    bt_label: RECT,
    bt_scan_btn: RECT,
    bt_box: RECT,
    div1_y: i32,
    autostart_row: RECT,
    debug_log_row: RECT,
    div2_y: i32,
    open_config_btn: RECT,
    exit_btn: RECT,
}

impl FlyoutLayout {
    fn compute(dpi: i32) -> Self {
        let w = s(LOGICAL_WIDTH, dpi);
        let pad = s(16, dpi);
        let inner_w = w - pad * 2;
        let header_h = s(64, dpi);

        Self {
            header: RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: header_h,
            },
            icon_rect: RECT {
                left: pad,
                top: s(18, dpi),
                right: pad + s(28, dpi),
                bottom: s(46, dpi),
            },
            title_rect: RECT {
                left: pad + s(38, dpi),
                top: s(12, dpi),
                right: w - pad - s(36, dpi),
                bottom: s(34, dpi),
            },
            version_rect: RECT {
                left: w - pad - s(36, dpi),
                top: s(14, dpi),
                right: w - pad,
                bottom: s(32, dpi),
            },
            status_dot_rect: RECT {
                left: pad + s(39, dpi),
                top: s(39, dpi),
                right: pad + s(47, dpi),
                bottom: s(47, dpi),
            },
            status_text_rect: RECT {
                left: pad + s(53, dpi),
                top: s(34, dpi),
                right: w - pad,
                bottom: s(52, dpi),
            },
            pause_btn: RECT {
                left: pad,
                top: s(74, dpi),
                right: w - pad,
                bottom: s(112, dpi),
            },
            mode_label: RECT {
                left: pad,
                top: s(122, dpi),
                right: w - pad,
                bottom: s(138, dpi),
            },
            mode_box: RECT {
                left: pad,
                top: s(140, dpi),
                right: w - pad,
                bottom: s(176, dpi),
            },
            bt_label: RECT {
                left: pad,
                top: s(188, dpi),
                right: w - pad - s(72, dpi),
                bottom: s(206, dpi),
            },
            bt_scan_btn: RECT {
                left: w - pad - s(68, dpi),
                top: s(184, dpi),
                right: w - pad,
                bottom: s(203, dpi),
            },
            bt_box: RECT {
                left: pad,
                top: s(208, dpi),
                right: w - pad,
                bottom: s(244, dpi),
            },
            div1_y: s(254, dpi),
            autostart_row: RECT {
                left: pad,
                top: s(260, dpi),
                right: w - pad,
                bottom: s(286, dpi),
            },
            debug_log_row: RECT {
                left: pad,
                top: s(288, dpi),
                right: w - pad,
                bottom: s(314, dpi),
            },
            div2_y: s(320, dpi),
            open_config_btn: RECT {
                left: pad,
                top: s(328, dpi),
                right: pad + (inner_w - s(8, dpi)) / 2,
                bottom: s(354, dpi),
            },
            exit_btn: RECT {
                left: pad + (inner_w - s(8, dpi)) / 2 + s(8, dpi),
                top: s(328, dpi),
                right: w - pad,
                bottom: s(354, dpi),
            },
        }
    }
}

fn mode_dropdown_rect(layout: &FlyoutLayout, dpi: i32) -> RECT {
    let row_h = s(32, dpi);
    let pad_v = s(4, dpi);
    RECT {
        left: layout.mode_box.left,
        top: layout.mode_box.bottom + s(3, dpi),
        right: layout.mode_box.right,
        bottom: layout.mode_box.bottom + s(3, dpi) + MODES.len() as i32 * row_h + pad_v * 2,
    }
}

fn bt_dropdown_rect(layout: &FlyoutLayout, dpi: i32, total_items: usize) -> RECT {
    let visible = total_items.clamp(1, MAX_BT_VISIBLE_ROWS) as i32;
    let row_h = s(30, dpi);
    let pad_v = s(4, dpi);
    let bottom = layout.bt_box.top - s(2, dpi);
    RECT {
        left: layout.bt_box.left,
        top: bottom - (visible * row_h + pad_v * 2),
        right: layout.bt_box.right,
        bottom,
    }
}

fn hit_test(
    x: i32,
    y: i32,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    bt_data: &BtMenuData,
) -> HitTarget {
    match ui.open_dropdown {
        OpenDropdown::Mode => {
            let pop = mode_dropdown_rect(layout, ui.dpi);
            if pt_in(&pop, x, y) {
                let row_h = s(32, ui.dpi);
                let rel_y = y - (pop.top + s(4, ui.dpi));
                if rel_y >= 0 {
                    let idx = (rel_y / row_h) as usize;
                    if idx < MODES.len() {
                        return HitTarget::ModeOption(idx);
                    }
                }
                return HitTarget::None;
            }
        }
        OpenDropdown::BtDevice => {
            let total_rows = bt_data.rows.len() + 1;
            let pop = bt_dropdown_rect(layout, ui.dpi, total_rows);
            if pt_in(&pop, x, y) {
                let row_h = s(30, ui.dpi);
                let rel_y = y - (pop.top + s(4, ui.dpi));
                if rel_y >= 0 {
                    let vis_idx = (rel_y / row_h) as usize;
                    let idx = ui.bt_scroll_offset + vis_idx;
                    if idx < bt_data.rows.len() {
                        return HitTarget::BtOption(idx);
                    } else if idx == bt_data.rows.len() {
                        return HitTarget::BtScanRow;
                    }
                }
                return HitTarget::None;
            }
        }
        OpenDropdown::None => {}
    }

    if pt_in(&layout.pause_btn, x, y) {
        HitTarget::PauseButton
    } else if pt_in(&layout.mode_box, x, y) {
        HitTarget::ModeBox
    } else if pt_in(&layout.bt_scan_btn, x, y) {
        HitTarget::BtScanButton
    } else if pt_in(&layout.bt_box, x, y) {
        HitTarget::BtBox
    } else if pt_in(&layout.autostart_row, x, y) {
        HitTarget::AutostartRow
    } else if pt_in(&layout.debug_log_row, x, y) {
        HitTarget::DebugLogRow
    } else if pt_in(&layout.open_config_btn, x, y) {
        HitTarget::OpenConfigBtn
    } else if pt_in(&layout.exit_btn, x, y) {
        HitTarget::ExitBtn
    } else {
        HitTarget::None
    }
}

unsafe extern "system" fn flyout_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_invalid() {
                if let Some(shared) = tray_state() {
                    paint_flyout(hwnd, hdc, shared);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let Some(shared) = tray_state() else {
                return LRESULT(0);
            };
            let bt_data = collect_bt_menu_data(shared);
            let changed = ACTIVE_UI.with(|cell| {
                let mut borrow = cell.borrow_mut();
                let Some(ui) = borrow.as_mut() else {
                    return false;
                };
                if !ui.tracking_mouse {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    ui.tracking_mouse = true;
                }
                let layout = FlyoutLayout::compute(ui.dpi);
                let next_hover = hit_test(x, y, ui, &layout, &bt_data);
                if ui.hover != next_hover {
                    ui.hover = next_hover;
                    true
                } else {
                    false
                }
            });
            if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let changed = ACTIVE_UI.with(|cell| {
                let mut borrow = cell.borrow_mut();
                let Some(ui) = borrow.as_mut() else {
                    return false;
                };
                ui.tracking_mouse = false;
                if ui.hover != HitTarget::None {
                    ui.hover = HitTarget::None;
                    true
                } else {
                    false
                }
            });
            if changed {
                let _ = InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
                let is_interactive = ACTIVE_UI.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .is_some_and(|ui| ui.hover != HitTarget::None)
                });
                let cursor_id = if is_interactive { IDC_HAND } else { IDC_ARROW };
                if let Ok(cur) = LoadCursorW(None, cursor_id) {
                    let _ = SetCursor(cur);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            if let Some(shared) = tray_state() {
                let bt_data = collect_bt_menu_data(shared);
                let total_rows = bt_data.rows.len() + 1;
                let max_offset = total_rows.saturating_sub(MAX_BT_VISIBLE_ROWS);
                let changed = ACTIVE_UI.with(|cell| {
                    let mut borrow = cell.borrow_mut();
                    let Some(ui) = borrow.as_mut() else {
                        return false;
                    };
                    if ui.open_dropdown != OpenDropdown::BtDevice || max_offset == 0 {
                        return false;
                    }
                    let next = if delta > 0 {
                        ui.bt_scroll_offset.saturating_sub(1)
                    } else {
                        (ui.bt_scroll_offset + 1).min(max_offset)
                    };
                    if next != ui.bt_scroll_offset {
                        ui.bt_scroll_offset = next;
                        true
                    } else {
                        false
                    }
                });
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            handle_flyout_click(hwnd, x, y);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if (wparam.0 as u16) == VK_ESCAPE.0 {
                let closed_dropdown = ACTIVE_UI.with(|cell| {
                    let mut borrow = cell.borrow_mut();
                    if let Some(ui) = borrow.as_mut() {
                        if ui.open_dropdown != OpenDropdown::None {
                            ui.open_dropdown = OpenDropdown::None;
                            return true;
                        }
                    }
                    false
                });
                if closed_dropdown {
                    let _ = InvalidateRect(hwnd, None, false);
                } else {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(shared) = tray_state() {
                shared.flyout_hwnd_raw.store(0, Ordering::Release);
                if let Ok(mut closed_at) = shared.last_flyout_closed.lock() {
                    *closed_at = Some(Instant::now());
                }
            }
            ACTIVE_UI.with(|cell| {
                if let Some(mut ui) = cell.borrow_mut().take() {
                    ui.free_fonts();
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn handle_flyout_click(hwnd: HWND, x: i32, y: i32) {
    let Some(shared) = tray_state() else {
        return;
    };
    let bt_data = collect_bt_menu_data(shared);
    let target = ACTIVE_UI.with(|cell| {
        let borrow = cell.borrow();
        let Some(ui) = borrow.as_ref() else {
            return HitTarget::None;
        };
        let layout = FlyoutLayout::compute(ui.dpi);
        hit_test(x, y, ui, &layout, &bt_data)
    });

    if matches!(target, HitTarget::None) {
        let dismissed_dropdown = ACTIVE_UI.with(|cell| {
            let mut borrow = cell.borrow_mut();
            if let Some(ui) = borrow.as_mut() {
                if ui.open_dropdown != OpenDropdown::None {
                    ui.open_dropdown = OpenDropdown::None;
                    return true;
                }
            }
            false
        });
        if dismissed_dropdown {
            let _ = InvalidateRect(hwnd, None, false);
        }
        return;
    }

    match target {
        HitTarget::PauseButton => {
            close_open_dropdown();
            toggle_pause(shared);
        }
        HitTarget::ModeBox => {
            ACTIVE_UI.with(|cell| {
                if let Some(ui) = cell.borrow_mut().as_mut() {
                    ui.open_dropdown = if ui.open_dropdown == OpenDropdown::Mode {
                        OpenDropdown::None
                    } else {
                        OpenDropdown::Mode
                    };
                }
            });
            let _ = InvalidateRect(hwnd, None, false);
        }
        HitTarget::ModeOption(idx) => {
            close_open_dropdown();
            if let Some(&mode) = MODES.get(idx) {
                trigger_mode_switch(shared, mode);
            }
            let _ = InvalidateRect(hwnd, None, false);
        }
        HitTarget::BtScanButton => {
            close_open_dropdown();
            spawn_bt_device_scan(shared);
        }
        HitTarget::BtBox => {
            ACTIVE_UI.with(|cell| {
                if let Some(ui) = cell.borrow_mut().as_mut() {
                    ui.bt_scroll_offset = 0;
                    ui.open_dropdown = if ui.open_dropdown == OpenDropdown::BtDevice {
                        OpenDropdown::None
                    } else {
                        OpenDropdown::BtDevice
                    };
                }
            });
            let _ = InvalidateRect(hwnd, None, false);
        }
        HitTarget::BtOption(idx) => {
            close_open_dropdown();
            if let Some(dev) = bt_data.rows.get(idx) {
                select_bt_device(shared, dev);
            }
            let _ = InvalidateRect(hwnd, None, false);
        }
        HitTarget::BtScanRow => {
            close_open_dropdown();
            spawn_bt_device_scan(shared);
        }
        HitTarget::AutostartRow => {
            close_open_dropdown();
            let cur = is_autostart_enabled();
            let _ = set_autostart_enabled(!cur);
            let _ = InvalidateRect(hwnd, None, false);
        }
        HitTarget::DebugLogRow => {
            let _ = DestroyWindow(hwnd);
            toggle_debug_log_window();
        }
        HitTarget::OpenConfigBtn => {
            let _ = DestroyWindow(hwnd);
            open_config_folder();
        }
        HitTarget::ExitBtn => {
            let _ = DestroyWindow(hwnd);
            shared.signal.request_exit();
        }
        HitTarget::None => {}
    }
}

fn close_open_dropdown() {
    ACTIVE_UI.with(|cell| {
        if let Some(ui) = cell.borrow_mut().as_mut() {
            ui.open_dropdown = OpenDropdown::None;
        }
    });
}

unsafe fn paint_flyout(hwnd: HWND, window_hdc: HDC, shared: &Arc<TraySharedState>) {
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let w = (client.right - client.left).max(1);
    let h = (client.bottom - client.top).max(1);

    let mem_dc = CreateCompatibleDC(window_hdc);
    if mem_dc.is_invalid() {
        return;
    }
    let mem_bmp: HBITMAP = CreateCompatibleBitmap(window_hdc, w, h);
    if mem_bmp.is_invalid() {
        let _ = DeleteDC(mem_dc);
        return;
    }
    let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0));
    let _ = SetBkMode(mem_dc, TRANSPARENT);

    let bt_data = collect_bt_menu_data(shared);
    let status_text = shared
        .status_text
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "AetherSora".to_owned());
    let selected_mode = shared
        .selected_mode
        .lock()
        .map(|g| *g)
        .unwrap_or(TransportMode::Auto);
    let is_paused = shared.paused.load(Ordering::Acquire);
    let hicon = shared
        .hicon_raw
        .lock()
        .ok()
        .map(|r| HICON(*r as *mut core::ffi::c_void))
        .unwrap_or_default();

    ACTIVE_UI.with(|cell| {
        let borrow = cell.borrow();
        let Some(ui) = borrow.as_ref() else {
            return;
        };
        let dpi = ui.dpi;
        let layout = FlyoutLayout::compute(dpi);

        fill_solid_rect(mem_dc, &client, BG_DEEP_SKY);
        paint_header(mem_dc, ui, &layout, hicon, &status_text, is_paused);
        paint_pause_button(mem_dc, ui, &layout, is_paused);
        paint_mode_selector(mem_dc, ui, &layout, selected_mode);
        paint_bt_selector(mem_dc, ui, &layout, &bt_data);

        draw_h_line(mem_dc, s(16, dpi), layout.div1_y, w - s(16, dpi), DIVIDER_LINE);

        paint_checkbox_row(
            mem_dc,
            ui,
            &layout.autostart_row,
            "Start with Windows",
            is_autostart_enabled(),
            ui.hover == HitTarget::AutostartRow,
        );
        paint_checkbox_row(
            mem_dc,
            ui,
            &layout.debug_log_row,
            "Show Live Debug Log",
            is_debug_log_visible(),
            ui.hover == HitTarget::DebugLogRow,
        );

        draw_h_line(mem_dc, s(16, dpi), layout.div2_y, w - s(16, dpi), DIVIDER_LINE);
        paint_footer(mem_dc, ui, &layout);

        match ui.open_dropdown {
            OpenDropdown::Mode => {
                paint_mode_dropdown_overlay(mem_dc, ui, &layout, selected_mode);
            }
            OpenDropdown::BtDevice => {
                paint_bt_dropdown_overlay(mem_dc, ui, &layout, &bt_data);
            }
            OpenDropdown::None => {}
        }
    });

    let _ = BitBlt(window_hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    let _ = SelectObject(mem_dc, old_bmp);
    let _ = DeleteObject(HGDIOBJ(mem_bmp.0));
    let _ = DeleteDC(mem_dc);
}

unsafe fn paint_header(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    hicon: HICON,
    status_text: &str,
    is_paused: bool,
) {
    let dpi = ui.dpi;
    fill_solid_rect(hdc, &layout.header, BG_HEADER);
    draw_h_line(hdc, 0, layout.header.bottom - 1, layout.header.right, BORDER_SUBTLE);

    let stars: [(i32, i32, i32, COLORREF); 7] = [
        (216, 11, 2, rgb(0x38, 0xBD, 0xF8)),
        (242, 22, 1, TEXT_MUTED),
        (264, 9, 2, ACCENT_GOLD),
        (289, 19, 1, ACCENT_SKY),
        (308, 12, 2, rgb(0x60, 0xA5, 0xFA)),
        (252, 46, 1, rgb(0x64, 0x74, 0x8B)),
        (312, 43, 2, ACCENT_SKY),
    ];
    for (lx, ly, sz, col) in stars {
        let px = s(lx, dpi);
        let py = s(ly, dpi);
        let psz = s(sz, dpi).max(1);
        let star_rc = RECT {
            left: px,
            top: py,
            right: px + psz,
            bottom: py + psz,
        };
        fill_solid_rect(hdc, &star_rc, col);
    }

    if !hicon.0.is_null() {
        let icon_w = layout.icon_rect.right - layout.icon_rect.left;
        let icon_h = layout.icon_rect.bottom - layout.icon_rect.top;
        let _ = DrawIconEx(
            hdc,
            layout.icon_rect.left,
            layout.icon_rect.top,
            hicon,
            icon_w,
            icon_h,
            0,
            None,
            DI_NORMAL,
        );
    }

    draw_text(
        hdc,
        ui.font_title,
        &layout.title_rect,
        "AetherSora",
        TEXT_PRIMARY,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    draw_text(
        hdc,
        ui.font_label,
        &layout.version_rect,
        "v0.3.1",
        ACCENT_SKY,
        DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let dot_color = if is_paused {
        ACCENT_GOLD
    } else if status_text.contains("Connected") || status_text.contains("48 kHz") {
        STATUS_CONNECTED
    } else {
        ACCENT_SKY
    };
    fill_circle(hdc, &layout.status_dot_rect, dot_color);

    draw_text(
        hdc,
        ui.font_label,
        &layout.status_text_rect,
        status_text,
        if is_paused { ACCENT_GOLD } else { TEXT_MUTED },
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
}

unsafe fn paint_pause_button(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    is_paused: bool,
) {
    let dpi = ui.dpi;
    let hovered = ui.hover == HitTarget::PauseButton && ui.open_dropdown == OpenDropdown::None;
    let (fill, border, fg, label) = if is_paused {
        (
            if hovered { ACCENT_GOLD_HOVER } else { ACCENT_GOLD },
            ACCENT_GOLD,
            TEXT_DARK,
            "Resume Receiver",
        )
    } else {
        (
            if hovered { BG_SURFACE_HOVER } else { BG_SURFACE },
            if hovered { ACCENT_SKY } else { BORDER_SUBTLE },
            TEXT_PRIMARY,
            "Pause Receiver",
        )
    };

    fill_round_rect(hdc, &layout.pause_btn, s(8, dpi), fill, border);

    let cy = (layout.pause_btn.top + layout.pause_btn.bottom) / 2;
    let icon_left = layout.pause_btn.left + s(96, dpi);
    if is_paused {
        draw_play_icon(hdc, icon_left, cy, dpi, fg);
    } else {
        draw_pause_icon(hdc, icon_left, cy, dpi, ACCENT_SKY);
    }

    let text_rc = RECT {
        left: icon_left + s(18, dpi),
        top: layout.pause_btn.top,
        right: layout.pause_btn.right - s(12, dpi),
        bottom: layout.pause_btn.bottom,
    };
    draw_text(
        hdc,
        ui.font_semi,
        &text_rc,
        label,
        fg,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn paint_mode_selector(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    selected_mode: TransportMode,
) {
    let dpi = ui.dpi;
    draw_text(
        hdc,
        ui.font_label,
        &layout.mode_label,
        "CONNECTION MODE",
        TEXT_MUTED,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let active = ui.open_dropdown == OpenDropdown::Mode;
    let hovered = ui.hover == HitTarget::ModeBox && ui.open_dropdown == OpenDropdown::None;
    let fill = if hovered || active { BG_SURFACE_HOVER } else { BG_SURFACE };
    let border = if active { ACCENT_SKY } else if hovered { rgb(0x3B, 0x82, 0xF6) } else { BORDER_SUBTLE };
    fill_round_rect(hdc, &layout.mode_box, s(8, dpi), fill, border);

    let text_rc = RECT {
        left: layout.mode_box.left + s(12, dpi),
        top: layout.mode_box.top,
        right: layout.mode_box.right - s(28, dpi),
        bottom: layout.mode_box.bottom,
    };
    draw_text(
        hdc,
        ui.font_body,
        &text_rc,
        mode_display_name(selected_mode),
        TEXT_PRIMARY,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    draw_chevron(
        hdc,
        layout.mode_box.right - s(16, dpi),
        (layout.mode_box.top + layout.mode_box.bottom) / 2,
        dpi,
        active,
        ACCENT_SKY,
    );
}

unsafe fn paint_bt_selector(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    bt_data: &BtMenuData,
) {
    let dpi = ui.dpi;
    draw_text(
        hdc,
        ui.font_label,
        &layout.bt_label,
        "BLUETOOTH DEVICE",
        TEXT_MUTED,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let scan_hover = ui.hover == HitTarget::BtScanButton && ui.open_dropdown == OpenDropdown::None;
    let scan_fill = if bt_data.scanning {
        BG_DISABLED
    } else if scan_hover {
        BG_SURFACE_HOVER
    } else {
        BG_SURFACE
    };
    let scan_border = if bt_data.scanning {
        DIVIDER_LINE
    } else if scan_hover {
        ACCENT_SKY
    } else {
        BORDER_SUBTLE
    };
    fill_round_rect(hdc, &layout.bt_scan_btn, s(6, dpi), scan_fill, scan_border);
    draw_text(
        hdc,
        ui.font_label,
        &layout.bt_scan_btn,
        if bt_data.scanning { "Scanning" } else { "Scan BT" },
        if bt_data.scanning { TEXT_MUTED } else { ACCENT_SKY },
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let active = ui.open_dropdown == OpenDropdown::BtDevice;
    let hovered = ui.hover == HitTarget::BtBox && ui.open_dropdown == OpenDropdown::None;
    let fill = if hovered || active { BG_SURFACE_HOVER } else { BG_SURFACE };
    let border = if active { ACCENT_SKY } else if hovered { rgb(0x3B, 0x82, 0xF6) } else { BORDER_SUBTLE };
    fill_round_rect(hdc, &layout.bt_box, s(8, dpi), fill, border);

    let text_rc = RECT {
        left: layout.bt_box.left + s(12, dpi),
        top: layout.bt_box.top,
        right: layout.bt_box.right - s(28, dpi),
        bottom: layout.bt_box.bottom,
    };
    let has_saved = bt_data.saved_id.is_some();
    draw_text(
        hdc,
        ui.font_body,
        &text_rc,
        &bt_data.display_label,
        if has_saved { TEXT_PRIMARY } else { TEXT_MUTED },
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
    );

    draw_chevron(
        hdc,
        layout.bt_box.right - s(16, dpi),
        (layout.bt_box.top + layout.bt_box.bottom) / 2,
        dpi,
        active,
        ACCENT_SKY,
    );
}

unsafe fn paint_checkbox_row(
    hdc: HDC,
    ui: &FlyoutUi,
    rc: &RECT,
    label: &str,
    checked: bool,
    hovered: bool,
) {
    let dpi = ui.dpi;
    let effective_hover = hovered && ui.open_dropdown == OpenDropdown::None;
    if effective_hover {
        fill_round_rect(hdc, rc, s(6, dpi), BG_SURFACE, BG_SURFACE);
    }

    let box_sz = s(15, dpi);
    let box_top = (rc.top + rc.bottom - box_sz) / 2;
    let box_rc = RECT {
        left: rc.left + s(6, dpi),
        top: box_top,
        right: rc.left + s(6, dpi) + box_sz,
        bottom: box_top + box_sz,
    };

    if checked {
        fill_round_rect(hdc, &box_rc, s(4, dpi), ACCENT_SKY, ACCENT_SKY);
        draw_checkmark(hdc, &box_rc, dpi, TEXT_DARK);
    } else {
        fill_round_rect(
            hdc,
            &box_rc,
            s(4, dpi),
            BG_DROPDOWN,
            if effective_hover { ACCENT_SKY } else { BORDER_SUBTLE },
        );
    }

    let text_rc = RECT {
        left: box_rc.right + s(10, dpi),
        top: rc.top,
        right: rc.right - s(6, dpi),
        bottom: rc.bottom,
    };
    draw_text(
        hdc,
        ui.font_body,
        &text_rc,
        label,
        TEXT_PRIMARY,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn paint_footer(hdc: HDC, ui: &FlyoutUi, layout: &FlyoutLayout) {
    let dpi = ui.dpi;
    let cfg_hover = ui.hover == HitTarget::OpenConfigBtn && ui.open_dropdown == OpenDropdown::None;
    fill_round_rect(
        hdc,
        &layout.open_config_btn,
        s(6, dpi),
        if cfg_hover { BG_SURFACE_HOVER } else { BG_SURFACE },
        if cfg_hover { ACCENT_SKY } else { BORDER_SUBTLE },
    );
    draw_text(
        hdc,
        ui.font_label,
        &layout.open_config_btn,
        "Open Config",
        TEXT_PRIMARY,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );

    let exit_hover = ui.hover == HitTarget::ExitBtn && ui.open_dropdown == OpenDropdown::None;
    fill_round_rect(
        hdc,
        &layout.exit_btn,
        s(6, dpi),
        if exit_hover { DANGER_BG_HOVER } else { BG_SURFACE },
        if exit_hover { DANGER_BORDER_HOVER } else { BORDER_SUBTLE },
    );
    draw_text(
        hdc,
        ui.font_label,
        &layout.exit_btn,
        "Exit AetherSora",
        if exit_hover { DANGER_TEXT_HOVER } else { TEXT_MUTED },
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

unsafe fn paint_mode_dropdown_overlay(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    selected_mode: TransportMode,
) {
    let dpi = ui.dpi;
    let pop = mode_dropdown_rect(layout, dpi);
    let moat = RECT {
        left: pop.left - s(2, dpi),
        top: pop.top - s(2, dpi),
        right: pop.right + s(2, dpi),
        bottom: pop.bottom + s(2, dpi),
    };
    fill_round_rect(hdc, &moat, s(10, dpi), BG_DEEP_SKY, BG_DEEP_SKY);
    fill_round_rect(hdc, &pop, s(8, dpi), BG_DROPDOWN, ACCENT_SKY);

    let row_h = s(32, dpi);
    let pad_v = s(4, dpi);
    for (idx, &mode) in MODES.iter().enumerate() {
        let row_rc = RECT {
            left: pop.left + s(4, dpi),
            top: pop.top + pad_v + (idx as i32) * row_h,
            right: pop.right - s(4, dpi),
            bottom: pop.top + pad_v + ((idx as i32) + 1) * row_h,
        };
        let is_selected = mode == selected_mode;
        let is_hovered = ui.hover == HitTarget::ModeOption(idx);

        if is_hovered {
            fill_round_rect(hdc, &row_rc, s(6, dpi), BG_DROPDOWN_HOVER, BG_DROPDOWN_HOVER);
        } else if is_selected {
            fill_round_rect(hdc, &row_rc, s(6, dpi), BG_SURFACE, BG_SURFACE);
        }

        if is_selected {
            let dot = RECT {
                left: row_rc.left + s(10, dpi),
                top: (row_rc.top + row_rc.bottom) / 2 - s(3, dpi),
                right: row_rc.left + s(16, dpi),
                bottom: (row_rc.top + row_rc.bottom) / 2 + s(3, dpi),
            };
            fill_circle(hdc, &dot, ACCENT_SKY);
        }

        let text_rc = RECT {
            left: row_rc.left + s(24, dpi),
            top: row_rc.top,
            right: row_rc.right - s(8, dpi),
            bottom: row_rc.bottom,
        };
        draw_text(
            hdc,
            if is_selected { ui.font_semi } else { ui.font_body },
            &text_rc,
            mode_display_name(mode),
            if is_selected { ACCENT_SKY } else { TEXT_PRIMARY },
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
}

unsafe fn paint_bt_dropdown_overlay(
    hdc: HDC,
    ui: &FlyoutUi,
    layout: &FlyoutLayout,
    bt_data: &BtMenuData,
) {
    let dpi = ui.dpi;
    let total_rows = bt_data.rows.len() + 1;
    let pop = bt_dropdown_rect(layout, dpi, total_rows);
    let moat = RECT {
        left: pop.left - s(2, dpi),
        top: pop.top - s(2, dpi),
        right: pop.right + s(2, dpi),
        bottom: pop.bottom + s(2, dpi),
    };
    fill_round_rect(hdc, &moat, s(10, dpi), BG_DEEP_SKY, BG_DEEP_SKY);
    fill_round_rect(hdc, &pop, s(8, dpi), BG_DROPDOWN, ACCENT_SKY);

    let row_h = s(30, dpi);
    let pad_v = s(4, dpi);
    let visible_rows = total_rows.min(MAX_BT_VISIBLE_ROWS);
    let has_scroll = total_rows > MAX_BT_VISIBLE_ROWS;
    let right_inset = if has_scroll { s(10, dpi) } else { s(4, dpi) };

    for vis in 0..visible_rows {
        let idx = ui.bt_scroll_offset + vis;
        if idx >= total_rows {
            break;
        }
        let row_rc = RECT {
            left: pop.left + s(4, dpi),
            top: pop.top + pad_v + (vis as i32) * row_h,
            right: pop.right - right_inset,
            bottom: pop.top + pad_v + ((vis as i32) + 1) * row_h,
        };

        if idx < bt_data.rows.len() {
            let dev = &bt_data.rows[idx];
            let is_selected = bt_data
                .saved_id
                .as_ref()
                .is_some_and(|sid| sid.eq_ignore_ascii_case(&dev.id));
            let is_hovered = ui.hover == HitTarget::BtOption(idx);

            if is_hovered {
                fill_round_rect(hdc, &row_rc, s(6, dpi), BG_DROPDOWN_HOVER, BG_DROPDOWN_HOVER);
            } else if is_selected {
                fill_round_rect(hdc, &row_rc, s(6, dpi), BG_SURFACE, BG_SURFACE);
            }

            if is_selected {
                let dot = RECT {
                    left: row_rc.left + s(10, dpi),
                    top: (row_rc.top + row_rc.bottom) / 2 - s(3, dpi),
                    right: row_rc.left + s(16, dpi),
                    bottom: (row_rc.top + row_rc.bottom) / 2 + s(3, dpi),
                };
                fill_circle(hdc, &dot, ACCENT_SKY);
            }

            let label = if dev.is_enabled {
                dev.name.as_str()
            } else {
                "Disabled Device"
            };
            let display = if dev.is_enabled {
                label.to_owned()
            } else {
                format!("{} (disabled)", dev.name)
            };

            let text_rc = RECT {
                left: row_rc.left + s(24, dpi),
                top: row_rc.top,
                right: row_rc.right - s(8, dpi),
                bottom: row_rc.bottom,
            };
            draw_text(
                hdc,
                if is_selected { ui.font_semi } else { ui.font_body },
                &text_rc,
                &display,
                if is_selected { ACCENT_SKY } else { TEXT_PRIMARY },
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        } else {
            let is_hovered = ui.hover == HitTarget::BtScanRow;
            if is_hovered {
                fill_round_rect(hdc, &row_rc, s(6, dpi), BG_DROPDOWN_HOVER, BG_DROPDOWN_HOVER);
            }
            let text_rc = RECT {
                left: row_rc.left + s(10, dpi),
                top: row_rc.top,
                right: row_rc.right - s(8, dpi),
                bottom: row_rc.bottom,
            };
            draw_text(
                hdc,
                ui.font_label,
                &text_rc,
                if bt_data.scanning {
                    "Scanning paired devices..."
                } else {
                    "Scan / Refresh Paired Devices..."
                },
                ACCENT_SKY,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }

    if has_scroll {
        let track_top = pop.top + s(6, dpi);
        let track_bot = pop.bottom - s(6, dpi);
        let track_h = (track_bot - track_top).max(1);
        let max_off = total_rows - MAX_BT_VISIBLE_ROWS;
        let thumb_h = ((track_h * MAX_BT_VISIBLE_ROWS as i32) / total_rows as i32).max(s(18, dpi));
        let thumb_y =
            track_top + ((track_h - thumb_h) * ui.bt_scroll_offset as i32) / max_off as i32;
        let thumb_rc = RECT {
            left: pop.right - s(6, dpi),
            top: thumb_y,
            right: pop.right - s(3, dpi),
            bottom: thumb_y + thumb_h,
        };
        fill_solid_rect(hdc, &thumb_rc, ACCENT_SKY);
    }
}

unsafe fn make_ui_font(logical_px: i32, weight: i32, dpi: i32) -> HFONT {
    CreateFontW(
        -s(logical_px, dpi),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        1,
        0,
        0,
        5,
        0,
        w!("Segoe UI"),
    )
}

unsafe fn fill_solid_rect(hdc: HDC, rc: &RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    let _ = FillRect(hdc, rc, brush);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn fill_round_rect(hdc: HDC, rc: &RECT, radius: i32, fill: COLORREF, border: COLORREF) {
    let brush: HBRUSH = CreateSolidBrush(fill);
    let pen: HPEN = CreatePen(PS_SOLID, 1, border);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, radius, radius);
    let _ = SelectObject(hdc, old_pen);
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(HGDIOBJ(pen.0));
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn fill_circle(hdc: HDC, rc: &RECT, fill: COLORREF) {
    let brush: HBRUSH = CreateSolidBrush(fill);
    let pen: HPEN = CreatePen(PS_NULL, 0, fill);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = Ellipse(hdc, rc.left, rc.top, rc.right, rc.bottom);
    let _ = SelectObject(hdc, old_pen);
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(HGDIOBJ(pen.0));
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn draw_h_line(hdc: HDC, x1: i32, y: i32, x2: i32, color: COLORREF) {
    let pen = CreatePen(PS_SOLID, 1, color);
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = MoveToEx(hdc, x1, y, None);
    let _ = LineTo(hdc, x2, y);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(pen.0));
}

unsafe fn draw_chevron(
    hdc: HDC,
    cx: i32,
    cy: i32,
    dpi: i32,
    up: bool,
    color: COLORREF,
) {
    let dx = s(4, dpi);
    let dy = s(2, dpi);
    let pts = if up {
        [
            POINT { x: cx - dx, y: cy + dy },
            POINT { x: cx, y: cy - dy },
            POINT { x: cx + dx, y: cy + dy },
        ]
    } else {
        [
            POINT { x: cx - dx, y: cy - dy },
            POINT { x: cx, y: cy + dy },
            POINT { x: cx + dx, y: cy - dy },
        ]
    };
    let pen = CreatePen(PS_SOLID, s(2, dpi).max(1), color);
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = Polyline(hdc, &pts);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(pen.0));
}

unsafe fn draw_checkmark(hdc: HDC, box_rc: &RECT, dpi: i32, color: COLORREF) {
    let w = box_rc.right - box_rc.left;
    let h = box_rc.bottom - box_rc.top;
    let pts = [
        POINT {
            x: box_rc.left + (w * 24) / 100,
            y: box_rc.top + (h * 52) / 100,
        },
        POINT {
            x: box_rc.left + (w * 44) / 100,
            y: box_rc.top + (h * 72) / 100,
        },
        POINT {
            x: box_rc.left + (w * 78) / 100,
            y: box_rc.top + (h * 30) / 100,
        },
    ];
    let pen = CreatePen(PS_SOLID, s(2, dpi).max(1), color);
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = Polyline(hdc, &pts);
    let _ = SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(pen.0));
}

unsafe fn draw_pause_icon(hdc: HDC, left: i32, cy: i32, dpi: i32, color: COLORREF) {
    let bar_w = s(3, dpi).max(2);
    let half_h = s(6, dpi);
    let gap = s(3, dpi);
    let r1 = RECT {
        left,
        top: cy - half_h,
        right: left + bar_w,
        bottom: cy + half_h,
    };
    let r2 = RECT {
        left: left + bar_w + gap,
        top: cy - half_h,
        right: left + bar_w * 2 + gap,
        bottom: cy + half_h,
    };
    fill_solid_rect(hdc, &r1, color);
    fill_solid_rect(hdc, &r2, color);
}

unsafe fn draw_play_icon(hdc: HDC, left: i32, cy: i32, dpi: i32, color: COLORREF) {
    let pts = [
        POINT {
            x: left,
            y: cy - s(6, dpi),
        },
        POINT {
            x: left + s(9, dpi),
            y: cy,
        },
        POINT {
            x: left,
            y: cy + s(6, dpi),
        },
    ];
    let brush = CreateSolidBrush(color);
    let pen = CreatePen(PS_NULL, 0, color);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = windows::Win32::Graphics::Gdi::Polygon(hdc, &pts);
    let _ = SelectObject(hdc, old_pen);
    let _ = SelectObject(hdc, old_brush);
    let _ = DeleteObject(HGDIOBJ(pen.0));
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn draw_text(
    hdc: HDC,
    font: HFONT,
    rc: &RECT,
    text: &str,
    color: COLORREF,
    flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
) {
    let old_font = SelectObject(hdc, HGDIOBJ(font.0));
    let _ = SetTextColor(hdc, color);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut mutable_rc = *rc;
    let _ = DrawTextW(hdc, &mut wide, &mut mutable_rc, flags);
    let _ = SelectObject(hdc, old_font);
}
