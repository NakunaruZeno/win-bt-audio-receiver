use std::collections::VecDeque;
use std::env;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
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
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, IsWindowVisible, LoadIconW,
    MoveWindow, PostMessageW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow,
    SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateMessage, CW_USEDEFAULT, HICON, HMENU,
    ICON_BIG, ICON_SMALL, IDI_APPLICATION, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING, MSG, SW_HIDE, SW_SHOW, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WM_SETFONT, WM_SETICON, WM_SIZE, WNDCLASSW, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE, WS_VSCROLL,
};

use crate::bluetooth::{list_paired_devices, PairedDevice};
use crate::config::{
    config_dir_path, load_saved_bt_device, save_bt_device, save_default_mode, TransportMode,
};
use crate::log::{self, WM_LOG_UPDATED};
use crate::signal::AppSignal;

pub const TRAY_WND_CLASS_NAME: PCWSTR = w!("AetherSoraTrayWndClass");
const LOG_WND_CLASS_NAME: PCWSTR = w!("AetherSoraDebugLogWndClass");

const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
const WM_TRAY_UPDATE_TIP: u32 = WM_APP + 2;
const WM_TRAY_SHOW_TOAST: u32 = WM_APP + 3;
pub const WM_TRAY_ALREADY_RUNNING: u32 = WM_APP + 5;

const TRAY_ICON_ID: u32 = 1;

const IDM_STATUS: usize = 1000;
const IDM_MODE_AUTO: usize = 1001;
const IDM_MODE_WIFI_APP: usize = 1002;
const IDM_MODE_WIFI_ADB: usize = 1003;
const IDM_MODE_BT: usize = 1004;
const IDM_AUTOSTART: usize = 1010;
const IDM_DEBUG_LOG: usize = 1011;
const IDM_OPEN_CONFIG: usize = 1012;
const IDM_BT_REFRESH: usize = 1020;
const IDM_BT_NONE: usize = 1021;
const IDM_EXIT: usize = 1099;
const IDM_BT_DEVICE_BASE: usize = 2000;
const MAX_BT_MENU_DEVICES: usize = 32;

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

struct TraySharedState {
    signal: Arc<AppSignal>,
    status_text: Mutex<String>,
    selected_mode: Mutex<TransportMode>,
    hwnd_raw: Mutex<isize>,
    hicon_raw: Mutex<isize>,
    pending_toasts: Mutex<VecDeque<PendingToast>>,
    bt_devices: Mutex<Vec<PairedDevice>>,
    last_menu_bt_devices: Mutex<Vec<PairedDevice>>,
    bt_scanning: AtomicBool,
    log_hwnd_raw: Mutex<isize>,
    log_edit_hwnd_raw: Mutex<isize>,
    log_font_raw: Mutex<isize>,
}

static TRAY_STATE: OnceLock<Arc<TraySharedState>> = OnceLock::new();

/// Pure Win32 System Tray icon backed by `Shell_NotifyIconW` (zero external crates).
/// Supports native Windows Toast notifications (`NIF_INFO`), inline Bluetooth device
/// selection, and a toggleable read-only Win32 Debug Log viewer.
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
            hwnd_raw: Mutex::new(0),
            hicon_raw: Mutex::new(0),
            pending_toasts: Mutex::new(VecDeque::with_capacity(4)),
            bt_devices: Mutex::new(Vec::new()),
            last_menu_bt_devices: Mutex::new(Vec::new()),
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

    pub fn show_toast(&self, title: impl Into<String>, body: impl Into<String>, kind: ToastKind) {
        enqueue_toast(&self.shared, title.into(), body.into(), kind);
    }

    pub fn set_bt_devices(&self, devices: Vec<PairedDevice>) {
        if let Ok(mut list) = self.shared.bt_devices.lock() {
            *list = devices;
        }
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

fn enqueue_toast(shared: &Arc<TraySharedState>, title: String, body: String, kind: ToastKind) {
    if let Ok(mut queue) = shared.pending_toasts.lock() {
        if queue.len() >= 4 {
            queue.pop_front();
        }
        queue.push_back(PendingToast { title, body, kind });
    }
    post_to_tray(shared, WM_TRAY_SHOW_TOAST);
}

unsafe fn run_tray_message_loop(shared: Arc<TraySharedState>, ready_tx: mpsc::SyncSender<()>) {
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
                show_tray_context_menu(hwnd);
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
            handle_menu_command(cmd_id);
            LRESULT(0)
        }
        WM_CLOSE => {
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

unsafe fn show_tray_context_menu(hwnd: HWND) {
    let Ok(hmenu) = CreatePopupMenu() else {
        return;
    };

    let (status_label, current_mode, cached_bt) = TRAY_STATE
        .get()
        .map(|s| {
            let st = s
                .status_text
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "AetherSora".to_owned());
            let md = s
                .selected_mode
                .lock()
                .map(|g| *g)
                .unwrap_or(TransportMode::Auto);
            let devs = s
                .bt_devices
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            (st, md, devs)
        })
        .unwrap_or_else(|| ("AetherSora".to_owned(), TransportMode::Auto, Vec::new()));

    let status_wide = to_wide(&status_label);
    let _ = AppendMenuW(
        hmenu,
        MF_STRING | MF_GRAYED,
        IDM_STATUS,
        PCWSTR(status_wide.as_ptr()),
    );
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

    append_radio_item(
        hmenu,
        IDM_MODE_AUTO,
        "Auto (Wi-Fi -> Bluetooth)",
        current_mode == TransportMode::Auto,
    );
    append_radio_item(
        hmenu,
        IDM_MODE_WIFI_APP,
        "Wi-Fi 5GHz (Companion App - No Dev Mode)",
        current_mode == TransportMode::WifiApp,
    );
    append_radio_item(
        hmenu,
        IDM_MODE_WIFI_ADB,
        "Wi-Fi 5GHz (ADB Wireless - Zero APK)",
        current_mode == TransportMode::WifiAdb,
    );
    append_radio_item(
        hmenu,
        IDM_MODE_BT,
        "Bluetooth A2DP Sink (Android & iOS)",
        current_mode == TransportMode::Bluetooth,
    );

    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

    if let Ok(bt_sub) = CreatePopupMenu() {
        let saved_bt = load_saved_bt_device();
        let saved_id = saved_bt.as_ref().map(|(id, _)| id.as_str());

        let menu_devices: Vec<PairedDevice> = if !cached_bt.is_empty() {
            cached_bt.into_iter().take(MAX_BT_MENU_DEVICES).collect()
        } else if let Some((id, name)) = saved_bt.clone() {
            vec![PairedDevice {
                id,
                name,
                is_enabled: true,
            }]
        } else {
            Vec::new()
        };

        if let Some(shared) = TRAY_STATE.get() {
            if let Ok(mut snap) = shared.last_menu_bt_devices.lock() {
                *snap = menu_devices.clone();
            }
        }

        if menu_devices.is_empty() {
            let _ = AppendMenuW(
                bt_sub,
                MF_STRING | MF_GRAYED,
                IDM_BT_NONE,
                w!("(No saved device - click Scan below)"),
            );
        } else {
            for (idx, dev) in menu_devices.iter().enumerate() {
                let is_selected = saved_id
                    .map(|sid| sid.eq_ignore_ascii_case(&dev.id))
                    .unwrap_or(false);
                let label = if dev.is_enabled {
                    dev.name.clone()
                } else {
                    format!("{} (disabled)", dev.name)
                };
                append_radio_item(bt_sub, IDM_BT_DEVICE_BASE + idx, &label, is_selected);
            }
        }

        let _ = AppendMenuW(bt_sub, MF_SEPARATOR, 0, PCWSTR::null());
        let is_scanning = TRAY_STATE
            .get()
            .map(|s| s.bt_scanning.load(Ordering::Acquire))
            .unwrap_or(false);
        let refresh_flags = if is_scanning {
            MF_STRING | MF_GRAYED
        } else {
            MF_STRING
        };
        let refresh_label = if is_scanning {
            w!("Scanning Paired Devices...")
        } else {
            w!("Scan / Refresh Paired Devices")
        };
        let _ = AppendMenuW(bt_sub, refresh_flags, IDM_BT_REFRESH, refresh_label);

        let _ = AppendMenuW(
            hmenu,
            MF_POPUP,
            bt_sub.0 as usize,
            w!("Bluetooth Device"),
        );
    }

    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

    let autostart_flags = if is_autostart_enabled() {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let _ = AppendMenuW(hmenu, autostart_flags, IDM_AUTOSTART, w!("Start with Windows"));

    let log_flags = if is_debug_log_visible() {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let _ = AppendMenuW(hmenu, log_flags, IDM_DEBUG_LOG, w!("Show Debug Log"));
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_OPEN_CONFIG, w!("Open Config Folder"));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(hmenu, MF_STRING, IDM_EXIT, w!("Exit AetherSora"));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        hmenu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(hmenu);
}

unsafe fn append_radio_item(hmenu: HMENU, id: usize, label: &str, checked: bool) {
    let flags = if checked {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    let wide = to_wide(label);
    let _ = AppendMenuW(hmenu, flags, id, PCWSTR(wide.as_ptr()));
}

fn handle_menu_command(cmd_id: usize) {
    let Some(shared) = TRAY_STATE.get() else {
        return;
    };

    if (IDM_BT_DEVICE_BASE..IDM_BT_DEVICE_BASE + MAX_BT_MENU_DEVICES).contains(&cmd_id) {
        let idx = cmd_id - IDM_BT_DEVICE_BASE;
        let chosen = shared
            .last_menu_bt_devices
            .lock()
            .ok()
            .and_then(|list| list.get(idx).cloned());
        if let Some(dev) = chosen {
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
                "[Tray] Selected Bluetooth device '{}' -> switching to Bluetooth A2DP Sink.",
                dev.name
            ));
            enqueue_toast(
                shared,
                "Bluetooth Device Selected".to_owned(),
                format!("Connecting to {}...", dev.name),
                ToastKind::Info,
            );
            trigger_mode_switch(shared, TransportMode::Bluetooth);
        }
        return;
    }

    match cmd_id {
        IDM_MODE_AUTO => trigger_mode_switch(shared, TransportMode::Auto),
        IDM_MODE_WIFI_APP => trigger_mode_switch(shared, TransportMode::WifiApp),
        IDM_MODE_WIFI_ADB => trigger_mode_switch(shared, TransportMode::WifiAdb),
        IDM_MODE_BT => trigger_mode_switch(shared, TransportMode::Bluetooth),
        IDM_BT_REFRESH => spawn_bt_device_scan(shared),
        IDM_AUTOSTART => {
            let currently_enabled = is_autostart_enabled();
            let _ = set_autostart_enabled(!currently_enabled);
        }
        IDM_DEBUG_LOG => unsafe {
            toggle_debug_log_window();
        },
        IDM_OPEN_CONFIG => {
            let dir = config_dir_path();
            let _ = Command::new("explorer.exe").arg(dir).spawn();
        }
        IDM_EXIT => {
            shared.signal.request_exit();
        }
        _ => {}
    }
}

fn trigger_mode_switch(shared: &Arc<TraySharedState>, mode: TransportMode) {
    if let Ok(mut m) = shared.selected_mode.lock() {
        *m = mode;
    }
    let _ = save_default_mode(mode);
    shared.signal.request_mode_switch(mode);
}

fn spawn_bt_device_scan(shared: &Arc<TraySharedState>) {
    if shared
        .bt_scanning
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

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
                            format!(
                                "Found {count} paired device(s). Right-click Tray -> Bluetooth Device to choose."
                            ),
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
        });
}

unsafe fn is_debug_log_visible() -> bool {
    let Some(shared) = TRAY_STATE.get() else {
        return false;
    };
    let raw = shared.log_hwnd_raw.lock().map(|g| *g).unwrap_or(0);
    if raw == 0 {
        return false;
    }
    IsWindowVisible(HWND(raw as *mut core::ffi::c_void)).as_bool()
}

unsafe fn toggle_debug_log_window() {
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

fn is_autostart_enabled() -> bool {
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

fn set_autostart_enabled(enable: bool) -> bool {
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

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn write_wide_buf<const N: usize>(dst: &mut [u16; N], text: &str) {
    dst.fill(0);
    for (i, ch) in text.encode_utf16().take(N.saturating_sub(1)).enumerate() {
        dst[i] = ch;
    }
}
