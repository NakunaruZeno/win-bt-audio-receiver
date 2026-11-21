use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_LOG_UPDATED: u32 = WM_APP + 6;
const MAX_LOG_LINES: usize = 250;

static LOG_BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static LOG_WINDOW_HWND: AtomicIsize = AtomicIsize::new(0);

fn buffer() -> &'static Mutex<VecDeque<String>> {
    LOG_BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LOG_LINES)))
}

/// Registers (or clears with `0`) the native Win32 Debug Log window handle
/// so new log lines trigger a live UI refresh only while the window exists.
pub fn set_log_window_hwnd(hwnd_raw: isize) {
    LOG_WINDOW_HWND.store(hwnd_raw, Ordering::Release);
}

fn push_line(line: &str) {
    if let Ok(mut buf) = buffer().lock() {
        for sub in line.split('\n') {
            let trimmed = sub.trim_end_matches('\r');
            if buf.len() >= MAX_LOG_LINES {
                buf.pop_front();
            }
            buf.push_back(trimmed.to_owned());
        }
    }

    let raw = LOG_WINDOW_HWND.load(Ordering::Acquire);
    if raw != 0 {
        unsafe {
            let _ = PostMessageW(
                HWND(raw as *mut core::ffi::c_void),
                WM_LOG_UPDATED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Logs an informational line to the in-memory ring buffer and to stdout (non-panicking).
pub fn info(msg: impl AsRef<str>) {
    let s = msg.as_ref();
    push_line(s);
    let _ = writeln!(io::stdout(), "{s}");
}

/// Logs an error/warning line to the in-memory ring buffer and to stderr (non-panicking).
pub fn error(msg: impl AsRef<str>) {
    let s = msg.as_ref();
    push_line(s);
    let _ = writeln!(io::stderr(), "{s}");
}

/// Returns all buffered log lines joined with `\r\n` for a Win32 multiline `EDIT` control.
pub fn snapshot_crlf() -> String {
    let Ok(buf) = buffer().lock() else {
        return String::new();
    };
    let mut out = String::with_capacity(buf.len() * 64);
    for (idx, line) in buf.iter().enumerate() {
        if idx > 0 {
            out.push_str("\r\n");
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_caps_lines_and_formats_crlf() {
        info("AetherSora log line 1");
        error("AetherSora log line 2");
        let text = snapshot_crlf();
        assert!(text.contains("AetherSora log line 1"));
        assert!(text.contains("AetherSora log line 2"));
        assert!(text.contains("\r\n"));
    }
}
