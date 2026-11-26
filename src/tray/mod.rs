pub mod flyout;

use std::collections::VecDeque;
use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, GetStockObject, COLOR_WINDOW, DEFAULT_GUI_FONT, HBRUSH, HFONT,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_LARGE_ICON, NIIF_NOSOUND,
    NIIF_RESPECT_QUIET_TIME, NIIF_USER, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    IsWindowVisible, LoadIconW, MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW,
    SendMessageW, SetForegroundWindow, SetProcessDPIAware, SetWindowTextW, ShowWindow,
    TranslateMessage, CW_USEDEFAULT, HICON, ICON_BIG, ICON_SMALL, IDI_APPLICATION, MSG, SW_HIDE,
    SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_LBUTTONUP, WM_RBUTTONUP, WM_SETFONT, WM_SETICON, WM_SIZE, WNDCLASSW, WS_CHILD,
    WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use crate::bluetooth::{list_paired_devices, PairedDevice};
use crate::config::{
    config_dir_path, save_bt_device, save_default_mode, TransportMode,
};
use crate::log::{self, WM_LOG_UPDATED};
use crate::signal::AppSignal;

pub const TRAY_WND_CLASS_NAME: PCWSTR = w!("AetherSoraTrayWndClass");
const LOG_WND_CLASS_NAME: PCWSTR = w!("AetherSoraDebugLogWndClass");

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_UPDATE_TIP: u32 = WM_APP + 2;
const WM_TRAY_SHOW_TOAST: u32 = WM_APP + 3;
const WM_TRAY_REFRESH_UI: u32 = WM_APP + 4;
pub const WM_TRAY_ALREADY_RUNNING: u32 = WM_APP + 5;

const TRAY_ICON_ID: u32 = 1;

pub const IDM_DEBUG_LOG: usize = 1011;
pub const IDM_TOGGLE_PAUSE: usize = 1015;
pub const IDM_OPEN_FLYOUT: usize = 1016;

const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_READONLY: u32 = 0x0800;
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warning,
}

struct PendingToast {
    title: String,
    body: String,
    kind: ToastKind,
}

pub(crate) struct TraySharedState {
    pub(crate) signal: Arc<AppSignal>,
    pub(crate) status_text: Mutex<String>,
    pub(crate) selected_mode: Mutex<TransportMode>,
    pub(crate) paused: AtomicBool,
    pub(crate) hwnd_raw: Mutex<isize>,
    pub(crate) hicon_raw: Mutex<isize>,
    pub(crate) flyout_hwnd_raw: AtomicIsize,
    pub(crate) last_flyout_closed: Mutex<Option<Instant>>,
    pending_toasts: Mutex<VecDeque<PendingToast>>,
    pub(crate) bt_devices: Mutex<Vec<PairedDevice>>,
    pub(crate) bt_scanning: AtomicBool,
    log_hwnd_raw: Mutex<isize>,
    log_edit_hwnd_raw: Mutex<isize>,
    log_font_raw: Mutex<isize>,
}

static TRAY_STATE: OnceLock<Arc<TraySharedState>> = OnceLock::new();

pub(crate) fn tray_state() -> Option<&'static Arc<TraySharedState>> {
    TRAY_STATE.get()
}

/// Pure Win32 System Tray controller backed by `Shell_NotifyIconW` (zero external crates).
/// Spawns the Celestial Sky flyout lazily on click with 0.00 MB idle UI overhead.
pub struct TrayController {
    shared: Arc<TraySharedState>,
    join_handle: Option<JoinHandle<()>>,
}

impl TrayController {
    pub fn start(signal: Arc<AppSignal>, initial_mode: TransportMode) -> Self {
        let shared = Arc::new(TraySharedState {
            signal,
            status_text: Mutex::new("AetherSora: Starting...".to_owned()),
            selected_mode: Mutex::new(initial_mode),
            paused: AtomicBool::new(false),
            hwnd_raw: Mutex::new(0),
            hicon_raw: Mutex::new(0),
            flyout_hwnd_raw: AtomicIsize::new(0),
            last_flyout_closed: Mutex::new(None),
            pending_toasts: Mutex::new(VecDeque::with_capacity(4)),
            bt_devices: Mutex::new(Vec::new()),
            bt_scanning: AtomicBool::new(false),
            log_hwnd_raw: Mutex::new(0),
            log_edit_hwnd_raw: Mutex::new(0),
            log_font_raw: Mutex::new(0),
        });
        let _ = TRAY_STATE.set(Arc::clone(&shared));

        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(1);
        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("aether-tray-ui".to_owned())
            .spawn(move || unsafe {
                run_tray_message_loop(thread_shared, ready_tx);
            })
            .expect("failed to spawn Win32 tray thread");

        let _ = ready_rx.recv();

        Self {
            shared,
            join_handle: Some(handle),
        }
    }

    pub fn update_status(&self, mode: TransportMode, status: impl Into<String>) {
        if let Ok(mut m) = self.shared.selected_mode.lock() {
            *m = mode;
        }
        if let Ok(mut s) = self.shared.status_text.lock() {
            *s = status.into();
        }
        post_to_tray(&self.shared, WM_TRAY_UPDATE_TIP);
    }

    pub fn set_paused_state(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
        post_to_tray(&self.shared, WM_TRAY_REFRESH_UI);
    }

    pub fn show_toast(&self, title: impl Into<String>, body: impl Into<String>, kind: ToastKind) {
        enqueue_toast(&self.shared, title.into(), body.into(), kind);
    }

    pub fn set_bt_devices(&self, devices: Vec<PairedDevice>) {
        if let Ok(mut list) = self.shared.bt_devices.lock() {
            *list = devices;
        }
        post_to_tray(&self.shared, WM_TRAY_REFRESH_UI);
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        post_to_tray(&self.shared, WM_CLOSE);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn post_to_tray(shared: &Arc<TraySharedState>, msg: u32) {
    if let Ok(raw) = shared.hwnd_raw.lock() {
        if *raw != 0 {
            unsafe {
                let _ = PostMessageW(
                    HWND(*raw as *mut core::ffi::c_void),
                    msg,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }
}

pub(crate) fn enqueue_toast(
    shared: &Arc<TraySharedState>,
    title: String,
    body: String,
    kind: ToastKind,
) {
    if let Ok(mut queue) = shared.pending_toasts.lock() {
        if queue.len() >= 4 {
            queue.pop_front();
        }
        queue.push_back(PendingToast { title, body, kind });
    }
    post_to_tray(shared, WM_TRAY_SHOW_TOAST);
}

unsafe fn run_tray_message_loop(shared: Arc<TraySharedState>, ready_tx: mpsc::SyncSender<()>) {
    let _ = SetProcessDPIAware();

    let Ok(hmodule) = GetModuleHandleW(None) else {
        let _ = ready_tx.send(());
        return;
    };
    let hinstance = HINSTANCE(hmodule.0);

    let wc = WNDCLASSW {
        lpfnWndProc: Some(tray_wnd_proc),
        hInstance: hinstance,
        lpszClassName: TRAY_WND_CLASS_NAME,
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);

    let Ok(hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        TRAY_WND_CLASS_NAME,
        w!("AetherSora"),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        None,
        None,
        hinstance,
        None,
    ) else {
        let _ = ready_tx.send(());
        return;
    };

    let icon_res_id = PCWSTR(std::ptr::without_provenance(1));
    let hicon = LoadIconW(hinstance, icon_res_id)
        .or_else(|_| LoadIconW(None, IDI_APPLICATION))
        .unwrap_or_default();

    if let Ok(mut raw) = shared.hwnd_raw.lock() {
        *raw = hwnd.0 as isize;
    }
    if let Ok(mut icon_raw) = shared.hicon_raw.lock() {
        *icon_raw = hicon.0 as isize;
    }

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon: hicon,
        ..Default::default()
    };
    write_wide_buf(&mut nid.szTip, "AetherSora — Starting...");
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);

    let _ = ready_tx.send(());

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    if let Ok(mut raw) = shared.hwnd_raw.lock() {
        *raw = 0;
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY_CALLBACK => {
            let mouse_evt = (lparam.0 & 0xFFFF) as u32;
            if mouse_evt == WM_RBUTTONUP || mouse_evt == WM_LBUTTONUP {
                if let Some(shared) = TRAY_STATE.get() {
                    flyout::toggle_flyout(shared, true);
                }
            }
            LRESULT(0)
        }
        WM_TRAY_UPDATE_TIP => {
            if let Some(shared) = TRAY_STATE.get() {
                if let Ok(status) = shared.status_text.lock() {
                    let mut nid = NOTIFYICONDATAW {
                        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: hwnd,
                        uID: TRAY_ICON_ID,
                        uFlags: NIF_TIP,
                        ..Default::default()
                    };
                    write_wide_buf(&mut nid.szTip, &status);
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                }
                flyout::request_repaint(shared);
            }
            LRESULT(0)
        }
        WM_TRAY_REFRESH_UI => {
            if let Some(shared) = TRAY_STATE.get() {
                flyout::request_repaint(shared);
            }
            LRESULT(0)
        }
        WM_TRAY_SHOW_TOAST => {
            if let Some(shared) = TRAY_STATE.get() {
                let next_toast = shared
                    .pending_toasts
                    .lock()
                    .ok()
                    .and_then(|mut q| q.pop_front());
                if let Some(toast) = next_toast {
                    display_tray_toast(hwnd, shared, &toast.title, &toast.body, toast.kind);
                }
            }
            LRESULT(0)
        }
        WM_TRAY_ALREADY_RUNNING => {
            if let Some(shared) = TRAY_STATE.get() {
                display_tray_toast(
                    hwnd,
                    shared,
                    "AetherSora",
                    "AetherSora is already running in the System Tray.",
                    ToastKind::Info,
                );
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd_id = wparam.0 & 0xFFFF;
            match cmd_id {
                IDM_DEBUG_LOG => toggle_debug_log_window(),
                IDM_TOGGLE_PAUSE => {
                    if let Some(shared) = TRAY_STATE.get() {
                        toggle_pause(shared);
                    }
                }
                IDM_OPEN_FLYOUT => {
                    if let Some(shared) = TRAY_STATE.get() {
                        flyout::open_flyout_anchored(shared);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(shared) = TRAY_STATE.get() {
                flyout::destroy_if_open(shared);
            }
            destroy_debug_log_window();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn display_tray_toast(
    hwnd: HWND,
    shared: &Arc<TraySharedState>,
    title: &str,
    body: &str,
    kind: ToastKind,
) {
    let hicon = shared
        .hicon_raw
        .lock()
        .ok()
        .map(|r| HICON(*r as *mut core::ffi::c_void))
        .unwrap_or_default();

    let info_flags = match kind {
        ToastKind::Info => NIIF_USER | NIIF_LARGE_ICON | NIIF_NOSOUND | NIIF_RESPECT_QUIET_TIME,
        ToastKind::Warning => NIIF_WARNING | NIIF_NOSOUND | NIIF_RESPECT_QUIET_TIME,
    };

    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: info_flags,
        hBalloonIcon: hicon,
        ..Default::default()
    };
    write_wide_buf(&mut nid.szInfoTitle, title);
    write_wide_buf(&mut nid.szInfo, body);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
}

pub(crate) fn mode_display_name(mode: TransportMode) -> &'static str {
    match mode {
        TransportMode::Auto => "Auto (Wi-Fi -> Bluetooth)",
        TransportMode::WifiApp => "Wi-Fi 5GHz (Companion App)",
        TransportMode::WifiAdb => "Wi-Fi 5GHz (ADB Wireless)",
        TransportMode::Bluetooth => "Bluetooth A2DP (Android & iOS)",
    }
}

pub(crate) fn trigger_mode_switch(shared: &Arc<TraySharedState>, mode: TransportMode) {
    if let Ok(mut m) = shared.selected_mode.lock() {
        *m = mode;
    }
    let _ = save_default_mode(mode);
    if shared.paused.load(Ordering::Acquire) {
        if let Ok(mut s) = shared.status_text.lock() {
            *s = format!("Paused — Ready ({})", mode.as_str());
        }
        shared.signal.request_mode_switch(mode);
        flyout::request_repaint(shared);
    } else {
        shared.signal.request_mode_switch(mode);
    }
}

pub(crate) fn toggle_pause(shared: &Arc<TraySharedState>) {
    let new_paused = !shared.signal.is_paused();
    shared.paused.store(new_paused, Ordering::Release);
    if let Ok(mut s) = shared.status_text.lock() {
        *s = if new_paused {
            "Pausing receiver...".to_owned()
        } else {
            "Resuming receiver...".to_owned()
        };
    }
    shared.signal.set_paused(new_paused);
    flyout::request_repaint(shared);
}

pub(crate) fn select_bt_device(shared: &Arc<TraySharedState>, dev: &PairedDevice) {
    if let Err(err) = save_bt_device(&dev.id, &dev.name) {
        log::error(format!("[Tray] Failed to save Bluetooth device: {err}"));
        enqueue_toast(
            shared,
            "Bluetooth Config Error".to_owned(),
            format!("Could not save device_id.cfg: {err}"),
            ToastKind::Warning,
        );
        return;
    }

    log::info(format!(
        "[Tray] Selected Bluetooth device '{}' -> connecting Bluetooth A2DP Sink.",
        dev.name
    ));
    enqueue_toast(
        shared,
        "Bluetooth Device Selected".to_owned(),
        format!("Connecting to {}...", dev.name),
        ToastKind::Info,
    );

    if let Ok(mut m) = shared.selected_mode.lock() {
        *m = TransportMode::Bluetooth;
    }
    let _ = save_default_mode(TransportMode::Bluetooth);
    shared.paused.store(false, Ordering::Release);
    shared.signal.request_mode_switch(TransportMode::Bluetooth);
    shared.signal.set_paused(false);
    flyout::request_repaint(shared);
}

pub(crate) fn spawn_bt_device_scan(shared: &Arc<TraySharedState>) {
    if shared
        .bt_scanning
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    flyout::request_repaint(shared);

    let shared_clone = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("aether-bt-scan".to_owned())
        .spawn(move || {
            log::info("[Tray] Scanning paired Bluetooth audio devices...");
            match list_paired_devices() {
                Ok(devices) => {
                    let count = devices.len();
                    if let Ok(mut list) = shared_clone.bt_devices.lock() {
                        *list = devices;
                    }
                    log::info(format!(
                        "[Tray] Bluetooth scan complete: found {count} paired audio device(s)."
                    ));
                    if count == 0 {
                        enqueue_toast(
                            &shared_clone,
                            "No Paired Bluetooth Devices".to_owned(),
                            "Pair your phone in Windows Settings -> Bluetooth & devices first."
                                .to_owned(),
                            ToastKind::Warning,
                        );
                    } else {
                        enqueue_toast(
                            &shared_clone,
                            "Bluetooth Scan Complete".to_owned(),
                            format!("Found {count} paired device(s). Pick your phone from the dropdown."),
                            ToastKind::Info,
                        );
                    }
                }
                Err(err) => {
                    log::error(format!("[Tray] Bluetooth scan failed: {err}"));
                    enqueue_toast(
                        &shared_clone,
                        "Bluetooth Scan Failed".to_owned(),
                        format!("Could not enumerate Bluetooth devices: {err}"),
                        ToastKind::Warning,
                    );
                }
            }
            shared_clone.bt_scanning.store(false, Ordering::Release);
            post_to_tray(&shared_clone, WM_TRAY_REFRESH_UI);
        });
}

pub(crate) fn open_config_folder() {
    let dir = config_dir_path();
    let _ = Command::new("explorer.exe").arg(dir).spawn();
}

pub(crate) unsafe fn is_debug_log_visible() -> bool {
    let Some(shared) = TRAY_STATE.get() else {
        return false;
    };
    let raw = shared.log_hwnd_raw.lock().map(|g| *g).unwrap_or(0);
    if raw == 0 {
        return false;
    }
    IsWindowVisible(HWND(raw as *mut core::ffi::c_void)).as_bool()
}

pub(crate) unsafe fn toggle_debug_log_window() {
    let Some(shared) = TRAY_STATE.get() else {
        return;
    };
    let existing = shared.log_hwnd_raw.lock().map(|g| *g).unwrap_or(0);
    if existing != 0 {
        let hwnd = HWND(existing as *mut core::ffi::c_void);
        if IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        } else {
            refresh_debug_log_contents(shared);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }

    let Ok(hmodule) = GetModuleHandleW(None) else {
        return;
    };
    let hinstance = HINSTANCE(hmodule.0);

    let wc = WNDCLASSW {
        lpfnWndProc: Some(debug_log_wnd_proc),
        hInstance: hinstance,
        hbrBackground: HBRUSH((COLOR_WINDOW.0 as usize + 1) as *mut core::ffi::c_void),
        lpszClassName: LOG_WND_CLASS_NAME,
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);

    let Ok(log_hwnd) = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        LOG_WND_CLASS_NAME,
        w!("AetherSora - Live Debug Log (closing hides window)"),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        780,
        440,
        None,
        None,
        hinstance,
        None,
    ) else {
        return;
    };

    if let Ok(icon_raw) = shared.hicon_raw.lock() {
        if *icon_raw != 0 {
            let _ = SendMessageW(log_hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(*icon_raw));
            let _ = SendMessageW(log_hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(*icon_raw));
        }
    }

    let mut rc = RECT::default();
    let _ = GetClientRect(log_hwnd, &mut rc);
    let edit_style = WINDOW_STYLE(
        WS_CHILD.0 | WS_VISIBLE.0 | WS_VSCROLL.0 | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY,
    );
    let Ok(edit_hwnd) = CreateWindowExW(
        WS_EX_CLIENTEDGE,
        w!("EDIT"),
        PCWSTR::null(),
        edit_style,
        0,
        0,
        rc.right - rc.left,
        rc.bottom - rc.top,
        log_hwnd,
        None,
        hinstance,
        None,
    ) else {
        let _ = DestroyWindow(log_hwnd);
        return;
    };

    let created_font = CreateFontW(
        -15,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        1,
        0,
        0,
        5,
        0,
        w!("Consolas"),
    );
    let font_handle = if !created_font.0.is_null() {
        if let Ok(mut f) = shared.log_font_raw.lock() {
            *f = created_font.0 as isize;
        }
        created_font.0 as usize
    } else {
        GetStockObject(DEFAULT_GUI_FONT).0 as usize
    };
    let _ = SendMessageW(edit_hwnd, WM_SETFONT, WPARAM(font_handle), LPARAM(1));

    if let Ok(mut h) = shared.log_hwnd_raw.lock() {
        *h = log_hwnd.0 as isize;
    }
    if let Ok(mut e) = shared.log_edit_hwnd_raw.lock() {
        *e = edit_hwnd.0 as isize;
    }
    log::set_log_window_hwnd(log_hwnd.0 as isize);

    refresh_debug_log_contents(shared);
    let _ = ShowWindow(log_hwnd, SW_SHOW);
    let _ = SetForegroundWindow(log_hwnd);
}

unsafe fn refresh_debug_log_contents(shared: &Arc<TraySharedState>) {
    let edit_raw = shared.log_edit_hwnd_raw.lock().map(|g| *g).unwrap_or(0);
    if edit_raw == 0 {
        return;
    }
    let edit_hwnd = HWND(edit_raw as *mut core::ffi::c_void);
    let text = log::snapshot_crlf();
    let wide = to_wide(&text);
    let char_len = wide.len().saturating_sub(1);
    let _ = SetWindowTextW(edit_hwnd, PCWSTR(wide.as_ptr()));
    let _ = SendMessageW(edit_hwnd, EM_SETSEL, WPARAM(char_len), LPARAM(char_len as isize));
    let _ = SendMessageW(edit_hwnd, EM_SCROLLCARET, WPARAM(0), LPARAM(0));
}

unsafe fn destroy_debug_log_window() {
    let Some(shared) = TRAY_STATE.get() else {
        return;
    };
    log::set_log_window_hwnd(0);
    let raw = shared
        .log_hwnd_raw
        .lock()
        .map(|mut g| {
            let v = *g;
            *g = 0;
            v
        })
        .unwrap_or(0);
    if let Ok(mut e) = shared.log_edit_hwnd_raw.lock() {
        *e = 0;
    }
    if raw != 0 {
        let _ = DestroyWindow(HWND(raw as *mut core::ffi::c_void));
    }
    if let Ok(mut f) = shared.log_font_raw.lock() {
        if *f != 0 {
            let _ = DeleteObject(HGDIOBJ(HFONT(*f as *mut core::ffi::c_void).0));
            *f = 0;
        }
    }
}

unsafe extern "system" fn debug_log_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SIZE => {
            if let Some(shared) = TRAY_STATE.get() {
                let edit_raw = shared.log_edit_hwnd_raw.lock().map(|g| *g).unwrap_or(0);
                if edit_raw != 0 {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rc);
                    let _ = MoveWindow(
                        HWND(edit_raw as *mut core::ffi::c_void),
                        0,
                        0,
                        rc.right - rc.left,
                        rc.bottom - rc.top,
                        true,
                    );
                }
            }
            LRESULT(0)
        }
        WM_LOG_UPDATED => {
            if IsWindowVisible(hwnd).as_bool() {
                if let Some(shared) = TRAY_STATE.get() {
                    refresh_debug_log_contents(shared);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_DESTROY => {
            log::set_log_window_hwnd(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub(crate) fn is_autostart_enabled() -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        let subkey = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, 0, KEY_QUERY_VALUE, &mut hkey) != ERROR_SUCCESS
        {
            return false;
        }
        let exists = RegQueryValueExW(hkey, w!("AetherSora"), None, None, None, None)
            == ERROR_SUCCESS;
        let _ = RegCloseKey(hkey);
        exists
    }
}

pub(crate) fn set_autostart_enabled(enable: bool) -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        let subkey = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey, 0, KEY_SET_VALUE, &mut hkey) != ERROR_SUCCESS {
            return false;
        }
        let ok = if enable {
            if let Ok(exe) = env::current_exe() {
                let cmd = format!("\"{}\" --tray", exe.display());
                let wide = to_wide(&cmd);
                let bytes = std::slice::from_raw_parts(
                    wide.as_ptr().cast::<u8>(),
                    wide.len() * std::mem::size_of::<u16>(),
                );
                RegSetValueExW(hkey, w!("AetherSora"), 0, REG_SZ, Some(bytes)) == ERROR_SUCCESS
            } else {
                false
            }
        } else {
            RegDeleteValueW(hkey, w!("AetherSora")) == ERROR_SUCCESS
        };
        let _ = RegCloseKey(hkey);
        ok
    }
}

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn write_wide_buf<const N: usize>(dst: &mut [u16; N], text: &str) {
    dst.fill(0);
    for (i, ch) in text.encode_utf16().take(N.saturating_sub(1)).enumerate() {
        dst[i] = ch;
    }
}
