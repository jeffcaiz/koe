//! Floating status overlay — a small pill at the bottom center of the screen.
//!
//! Shows recording status, interim ASR text, and processing state.
//! Windows implementation using Win32 layered window via windows-sys.

use std::sync::mpsc;
use std::thread;

/// Messages sent to the overlay thread.
pub enum OverlayMsg {
    UpdateState(String),
    UpdateInterimText(String),
    Dismiss,
}

static TX: std::sync::Mutex<Option<mpsc::Sender<OverlayMsg>>> = std::sync::Mutex::new(None);

/// Initialize the overlay. Spawns a background thread with its own window.
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

/// Update the overlay state (recording, correcting, idle, etc.)
pub fn update_state(state: &str) {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::UpdateState(state.to_string()));
    }
}

/// Update the interim text shown during recording.
pub fn update_interim_text(text: &str) {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::UpdateInterimText(text.to_string()));
    }
}

/// Dismiss the overlay.
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

    const WINDOW_WIDTH: i32 = 400;
    const WINDOW_HEIGHT: i32 = 60;
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 50;

    struct OverlayState {
        status_text: String,
        interim_text: String,
        bg_color: u32, // COLORREF (0x00BBGGRR)
        visible: bool,
        hwnd: HWND,
        dismiss_at: Option<std::time::Instant>,
    }

    static mut STATE: Option<OverlayState> = None;
    static mut RX: Option<mpsc::Receiver<OverlayMsg>> = None;

    fn status_for_state(state: &str) -> (&str, u32) {
        match state {
            s if s.starts_with("recording") => ("Listening...", 0x002828E0),   // red
            "connecting_asr" => ("Connecting...", 0x0028C8F0),                  // yellow
            "finalizing_asr" => ("Recognizing...", 0x00F0C858),                 // blue
            "correcting" => ("Thinking...", 0x00F09A58),                        // purple
            s if s.starts_with("preparing_paste") || s == "pasting" => {
                ("Done!", 0x0048D848)                                           // green
            }
            "failed" | "error" => ("Error", 0x002828E0),                        // red
            _ => ("", 0x00404040),                                              // gray
        }
    }

    /// Encode a Rust string as a null-terminated UTF-16 buffer.
    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        unsafe {
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

            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - WINDOW_WIDTH) / 2;
            let y = screen_h - WINDOW_HEIGHT - 80;

            let window_name = to_wide("Koe");
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_POPUP,
                x,
                y,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                std::ptr::null_mut(), // parent
                std::ptr::null_mut(), // menu
                hinstance,
                std::ptr::null(),
            );

            SetLayeredWindowAttributes(hwnd, 0, 220, LWA_ALPHA);

            STATE = Some(OverlayState {
                status_text: String::new(),
                interim_text: String::new(),
                bg_color: 0x00404040,
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
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TIMER if wparam == TIMER_ID => {
                poll_messages();
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
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
                        if !state.visible {
                            continue;
                        }
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(800),
                        );
                    } else if s.starts_with("preparing_paste") || s == "pasting" {
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(1000),
                        );
                        state.status_text = text.to_string();
                        state.bg_color = color;
                        show_window(state);
                    } else {
                        state.dismiss_at = None;
                        state.status_text = text.to_string();
                        state.bg_color = color;
                        if !text.is_empty() {
                            show_window(state);
                        }
                    }
                    needs_repaint = true;
                }
                OverlayMsg::UpdateInterimText(text) => {
                    state.interim_text = text;
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
            None => {
                EndPaint(hwnd, &ps);
                return;
            }
        };

        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);

        // Background
        let bg_brush = CreateSolidBrush(state.bg_color);
        FillRect(hdc, &rect, bg_brush);
        DeleteObject(bg_brush);

        // White text
        SetTextColor(hdc, 0x00FFFFFF);
        SetBkMode(hdc, TRANSPARENT as i32);

        // Status text (bold, 20px)
        let font_name = to_wide("Segoe UI");
        let font = CreateFontW(
            20, 0, 0, 0,
            700, // bold
            0, 0, 0,
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
            left: 16,
            top: 6,
            right: rect.right - 16,
            bottom: 30,
        };
        DrawTextW(
            hdc,
            status_buf.as_mut_ptr(),
            status_buf.len() as i32 - 1, // exclude null terminator
            &mut status_rect,
            DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        // Interim text (regular, 15px)
        if !state.interim_text.is_empty() {
            let small_font = CreateFontW(
                15, 0, 0, 0,
                400,
                0, 0, 0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                CLEARTYPE_QUALITY as u32,
                (DEFAULT_PITCH | FF_SWISS) as u32,
                font_name.as_ptr(),
            );
            SelectObject(hdc, small_font);

            let display_text = if state.interim_text.chars().count() > 50 {
                let start = state.interim_text.char_indices()
                    .rev()
                    .nth(49)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                format!("...{}", &state.interim_text[start..])
            } else {
                state.interim_text.clone()
            };

            let mut interim_buf = to_wide(&display_text);
            let mut interim_rect = RECT {
                left: 16,
                top: 32,
                right: rect.right - 16,
                bottom: rect.bottom - 4,
            };
            SetTextColor(hdc, 0x00E0E0E0);
            DrawTextW(
                hdc,
                interim_buf.as_mut_ptr(),
                interim_buf.len() as i32 - 1,
                &mut interim_rect,
                DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS,
            );

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
