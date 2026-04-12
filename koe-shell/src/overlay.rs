//! Floating status overlay — a compact pill at the bottom center of the screen.
//!
//! Design: dark rounded pill with a colored status dot on the left,
//! status text, and interim ASR text below.

use std::sync::mpsc;
use std::thread;

pub enum OverlayMsg {
    UpdateState(String),
    UpdateInterimText(String),
    Dismiss,
}

static TX: std::sync::Mutex<Option<mpsc::Sender<OverlayMsg>>> = std::sync::Mutex::new(None);

pub fn init() {
    let (tx, rx) = mpsc::channel();
    {
        let mut slot = TX.lock().unwrap();
        *slot = Some(tx);
    }
    thread::spawn(move || {
        platform::run_overlay(rx);
    });
}

pub fn update_state(state: &str) {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::UpdateState(state.to_string()));
    }
}

pub fn update_interim_text(text: &str) {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::UpdateInterimText(text.to_string()));
    }
}

#[allow(dead_code)]
pub fn dismiss() {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::Dismiss);
    }
}

#[cfg(windows)]
mod platform {
    use super::OverlayMsg;
    use std::sync::mpsc;

    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    // Monitor APIs
    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    extern "system" {
        fn MonitorFromWindow(hwnd: HWND, dwflags: u32) -> *mut std::ffi::c_void;
        fn GetMonitorInfoW(hmonitor: *mut std::ffi::c_void, lpmi: *mut MONITORINFO) -> BOOL;
        fn SetProcessDpiAwarenessContext(value: isize) -> BOOL;
        fn GetDpiForWindow(hwnd: HWND) -> u32;
    }

    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

    #[repr(C)]
    struct MONITORINFO {
        cbSize: u32,
        rcMonitor: RECT,
        rcWork: RECT,
        dwFlags: u32,
    }

    // Base dimensions at 96 DPI (100% scaling)
    const BASE_DPI: f32 = 96.0;
    const BASE_PILL_WIDTH: f32 = 280.0;
    const BASE_PILL_HEIGHT_SMALL: f32 = 36.0;
    const BASE_PILL_HEIGHT_LARGE: f32 = 54.0;
    const BASE_CORNER_RADIUS: f32 = 18.0;
    const BASE_DOT_RADIUS: f32 = 5.0;
    const BASE_STATUS_FONT: f32 = 16.0;
    const BASE_INTERIM_FONT: f32 = 13.0;

    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 50;
    const BG_COLOR: u32 = 0x00302828;    // dark charcoal (BGR)

    struct OverlayState {
        status_text: String,
        interim_text: String,
        dot_color: u32,
        visible: bool,
        hwnd: HWND,
        dismiss_at: Option<std::time::Instant>,
    }

    static mut STATE: Option<OverlayState> = None;
    static mut RX: Option<mpsc::Receiver<OverlayMsg>> = None;

    /// Returns (status_text, dot_color_bgr).
    fn status_for_state(state: &str) -> (&str, u32) {
        match state {
            s if s.starts_with("recording") => ("Listening...", 0x004040FF),     // red dot
            "connecting_asr" => ("Connecting...", 0x0040CCFF),                    // orange dot
            "finalizing_asr" => ("Recognizing...", 0x00FFCC40),                   // blue dot
            "correcting" => ("Thinking...", 0x00FF9060),                          // purple dot
            s if s.starts_with("preparing_paste") || s == "pasting" => {
                ("Done!", 0x0060DD60)                                             // green dot
            }
            "failed" | "error" => ("Error", 0x004040FF),                          // red dot
            _ => ("", 0x00808080),
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Get DPI scale factor for the overlay window. Returns 1.0 at 96 DPI, 1.5 at 144, 2.0 at 192, etc.
    unsafe fn dpi_scale(hwnd: HWND) -> f32 {
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 { 1.0 } else { dpi as f32 / BASE_DPI }
    }

    /// Scale a base dimension by DPI.
    fn scaled(base: f32, scale: f32) -> i32 {
        (base * scale).round() as i32
    }

    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        unsafe {
            // Declare per-monitor DPI awareness so we get real pixel sizes
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            RX = Some(rx);

            let class_name = to_wide("KoeOverlay");
            let hinstance = GetModuleHandleW(std::ptr::null());

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);

            // Initial position (will be updated when shown)
            let x = 0;
            let y = 0;

            let window_name = to_wide("Koe");
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_POPUP,
                x, y,
                BASE_PILL_WIDTH as i32, BASE_PILL_HEIGHT_SMALL as i32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );

            // Semi-transparent
            SetLayeredWindowAttributes(hwnd, 0, 230, LWA_ALPHA);

            // Rounded corners
            let rgn = CreateRoundRectRgn(0, 0, BASE_PILL_WIDTH as i32, BASE_PILL_HEIGHT_SMALL as i32, BASE_CORNER_RADIUS as i32, BASE_CORNER_RADIUS as i32);
            SetWindowRgn(hwnd, rgn, 1);

            STATE = Some(OverlayState {
                status_text: String::new(),
                interim_text: String::new(),
                dot_color: 0x00808080,
                visible: false,
                hwnd,
                dismiss_at: None,
            });

            SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None);

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TIMER if wparam == TIMER_ID => { poll_messages(); 0 }
            WM_PAINT => { paint(hwnd); 0 }
            WM_DESTROY => { PostQuitMessage(0); 0 }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn poll_messages() {
        let rx = match RX.as_ref() {
            Some(r) => r,
            None => return,
        };

        let mut needs_repaint = false;

        while let Ok(msg) = rx.try_recv() {
            let state = STATE.as_mut().unwrap();
            match msg {
                OverlayMsg::UpdateState(s) => {
                    let (text, color) = status_for_state(&s);

                    if s == "idle" || s == "completed" {
                        if !state.visible { continue; }
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(600),
                        );
                    } else if s.starts_with("preparing_paste") || s == "pasting" {
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(800),
                        );
                        state.status_text = text.to_string();
                        state.dot_color = color;
                        state.interim_text.clear();
                        resize_pill(state);
                        show_window(state);
                    } else {
                        state.dismiss_at = None;
                        state.status_text = text.to_string();
                        state.dot_color = color;
                        if !text.is_empty() {
                            resize_pill(state);
                            show_window(state);
                        }
                    }
                    needs_repaint = true;
                }
                OverlayMsg::UpdateInterimText(text) => {
                    let had_interim = !state.interim_text.is_empty();
                    state.interim_text = text;
                    let has_interim = !state.interim_text.is_empty();
                    if had_interim != has_interim {
                        resize_pill(state);
                    }
                    needs_repaint = true;
                }
                OverlayMsg::Dismiss => {
                    hide_window(state);
                    needs_repaint = false;
                }
            }
        }

        let state = STATE.as_mut().unwrap();
        if let Some(dismiss_at) = state.dismiss_at {
            if std::time::Instant::now() >= dismiss_at {
                state.dismiss_at = None;
                state.interim_text.clear();
                hide_window(state);
                return;
            }
        }

        if needs_repaint {
            InvalidateRect(state.hwnd, std::ptr::null(), 1);
        }
    }

    /// Get the working area of the monitor where the foreground window is.
    unsafe fn active_monitor_rect() -> RECT {
        let fg = GetForegroundWindow();
        let monitor = if fg.is_null() {
            MonitorFromWindow(std::ptr::null_mut(), MONITOR_DEFAULTTOPRIMARY)
        } else {
            MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST)
        };

        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            info.rcWork // working area (excludes taskbar)
        } else {
            // Fallback to primary screen
            RECT {
                left: 0, top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }

    /// Resize and reposition the pill on the active monitor, DPI-aware.
    unsafe fn resize_pill(state: &OverlayState) {
        let s = dpi_scale(state.hwnd);
        let w = scaled(BASE_PILL_WIDTH, s);
        let h = if state.interim_text.is_empty() {
            scaled(BASE_PILL_HEIGHT_SMALL, s)
        } else {
            scaled(BASE_PILL_HEIGHT_LARGE, s)
        };
        let corner = scaled(BASE_CORNER_RADIUS, s);

        let mon = active_monitor_rect();
        let mon_w = mon.right - mon.left;
        let x = mon.left + (mon_w - w) / 2;
        let y = mon.bottom - h - scaled(40.0, s);

        SetWindowPos(
            state.hwnd, HWND_TOPMOST,
            x, y, w, h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );

        let rgn = CreateRoundRectRgn(0, 0, w, h, corner, corner);
        SetWindowRgn(state.hwnd, rgn, 1);
    }

    unsafe fn show_window(state: &mut OverlayState) {
        if !state.visible {
            ShowWindow(state.hwnd, SW_SHOWNOACTIVATE);
            state.visible = true;
        }
    }

    unsafe fn hide_window(state: &mut OverlayState) {
        if state.visible {
            ShowWindow(state.hwnd, SW_HIDE);
            state.visible = false;
            state.status_text.clear();
            state.interim_text.clear();
        }
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);

        let state = match STATE.as_ref() {
            Some(s) => s,
            None => { EndPaint(hwnd, &ps); return; }
        };

        let s = dpi_scale(hwnd);

        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);

        // Dark background
        let bg_brush = CreateSolidBrush(BG_COLOR);
        FillRect(hdc, &rect, bg_brush);
        DeleteObject(bg_brush);

        // Status dot
        let dot_r = scaled(BASE_DOT_RADIUS, s);
        let dot_brush = CreateSolidBrush(state.dot_color);
        let old_brush = SelectObject(hdc, dot_brush);
        let null_pen = GetStockObject(NULL_PEN);
        let old_pen = SelectObject(hdc, null_pen);
        let dot_cx = scaled(18.0, s);
        let dot_cy = if state.interim_text.is_empty() {
            rect.bottom / 2
        } else {
            scaled(14.0, s)
        };
        Ellipse(hdc, dot_cx - dot_r, dot_cy - dot_r, dot_cx + dot_r, dot_cy + dot_r);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(dot_brush);

        // Text setup
        SetTextColor(hdc, 0x00FFFFFF);
        SetBkMode(hdc, TRANSPARENT as i32);

        let font_name = to_wide("Segoe UI");
        let text_left = scaled(30.0, s);
        let pad_right = scaled(12.0, s);

        // Status text
        let font = CreateFontW(
            scaled(BASE_STATUS_FONT, s), 0, 0, 0,
            600, 0, 0, 0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_SWISS) as u32,
            font_name.as_ptr(),
        );
        let old_font = SelectObject(hdc, font);

        let mut status_buf = to_wide(&state.status_text);
        let mut status_rect = RECT {
            left: text_left,
            top: if state.interim_text.is_empty() { 0 } else { scaled(4.0, s) },
            right: rect.right - pad_right,
            bottom: if state.interim_text.is_empty() { rect.bottom } else { scaled(24.0, s) },
        };
        DrawTextW(hdc, status_buf.as_mut_ptr(), status_buf.len() as i32 - 1, &mut status_rect, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);

        // Interim text
        if !state.interim_text.is_empty() {
            let small_font = CreateFontW(
                scaled(BASE_INTERIM_FONT, s), 0, 0, 0,
                400, 0, 0, 0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                (DEFAULT_PITCH | FF_SWISS) as u32,
                font_name.as_ptr(),
            );
            SelectObject(hdc, small_font);

            let display_text = if state.interim_text.chars().count() > 40 {
                let start = state.interim_text.char_indices()
                    .rev().nth(39).map(|(i, _)| i).unwrap_or(0);
                format!("…{}", &state.interim_text[start..])
            } else {
                state.interim_text.clone()
            };

            let mut interim_buf = to_wide(&display_text);
            let mut interim_rect = RECT {
                left: text_left,
                top: scaled(28.0, s),
                right: rect.right - pad_right,
                bottom: rect.bottom - scaled(4.0, s),
            };
            SetTextColor(hdc, 0x00B0B0B0);
            DrawTextW(hdc, interim_buf.as_mut_ptr(), interim_buf.len() as i32 - 1, &mut interim_rect, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);

            DeleteObject(small_font);
        }

        SelectObject(hdc, old_font);
        DeleteObject(font);
        EndPaint(hwnd, &ps);
    }
}

#[cfg(not(windows))]
mod platform {
    use super::OverlayMsg;
    use std::sync::mpsc;

    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        log::info!("overlay: no visual overlay on this platform (logging only)");
        while let Ok(msg) = rx.recv() {
            match msg {
                OverlayMsg::UpdateState(s) => log::info!("overlay state: {s}"),
                OverlayMsg::UpdateInterimText(t) => log::debug!("overlay interim: {t}"),
                OverlayMsg::Dismiss => log::debug!("overlay dismissed"),
            }
        }
    }
}
