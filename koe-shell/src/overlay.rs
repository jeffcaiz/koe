//! Floating status overlay — a small pill at the bottom center of the screen.
//!
//! Shows recording status, interim ASR text, and processing state.
//! Windows implementation using Win32 layered window.

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
pub fn dismiss() {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::Dismiss);
    }
}

#[cfg(windows)]
mod platform {
    use super::OverlayMsg;
    use std::sync::mpsc;

    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const WINDOW_WIDTH: i32 = 400;
    const WINDOW_HEIGHT: i32 = 60;
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 50;

    // Custom message to wake the window proc
    const WM_OVERLAY_UPDATE: u32 = WM_USER + 1;

    struct OverlayState {
        status_text: String,
        interim_text: String,
        accent_color: COLORREF,
        visible: bool,
        hwnd: HWND,
        dismiss_at: Option<std::time::Instant>,
    }

    static mut STATE: Option<OverlayState> = None;
    static mut RX: Option<mpsc::Receiver<OverlayMsg>> = None;

    fn status_for_state(state: &str) -> (&str, COLORREF) {
        match state {
            s if s.starts_with("recording") => ("Listening...", COLORREF(0x002828E0)), // red (BGR)
            "connecting_asr" => ("Connecting...", COLORREF(0x0028C8F0)),                // yellow
            "finalizing_asr" => ("Recognizing...", COLORREF(0x00F0C858)),               // blue
            "correcting" => ("Thinking...", COLORREF(0x00F09A58)),                      // purple-blue
            s if s.starts_with("preparing_paste") || s == "pasting" => {
                ("Done!", COLORREF(0x0048D848))                                         // green
            }
            "failed" | "error" => ("Error", COLORREF(0x002828E0)),                      // red
            _ => ("", COLORREF(0x00404040)),                                            // gray
        }
    }

    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        unsafe {
            RX = Some(rx);

            let class_name = w!("KoeOverlay");
            let hinstance = GetModuleHandleW(None).unwrap();

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                hbrBackground: HBRUSH(std::ptr::null_mut()),
                ..Default::default()
            };
            RegisterClassExW(&wc);

            // Position at bottom center of primary monitor
            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - WINDOW_WIDTH) / 2;
            let y = screen_h - WINDOW_HEIGHT - 80; // 80px above bottom

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class_name,
                w!("Koe"),
                WS_POPUP,
                x,
                y,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                None,
                None,
                Some(HINSTANCE(hinstance.0)),
                None,
            )
            .unwrap();

            // Set window opacity (220/255 ≈ 86%)
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 220, LWA_ALPHA);

            STATE = Some(OverlayState {
                status_text: String::new(),
                interim_text: String::new(),
                accent_color: COLORREF(0x00404040),
                visible: false,
                hwnd,
                dismiss_at: None,
            });

            // Timer to poll for messages from the channel
            SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None);

            // Message loop
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
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
            WM_TIMER if wparam.0 == TIMER_ID => {
                poll_messages();
                LRESULT(0)
            }
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
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
                        // Don't hide immediately — show "Done!" briefly
                        if !state.visible {
                            continue;
                        }
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(800),
                        );
                    } else if s.starts_with("preparing_paste") || s == "pasting" {
                        // Show "Done!" then auto-dismiss
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(1000),
                        );
                        state.status_text = text.to_string();
                        state.accent_color = color;
                        show_window(state);
                    } else {
                        state.dismiss_at = None;
                        state.status_text = text.to_string();
                        state.accent_color = color;
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

        // Check auto-dismiss timer
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
            let hwnd = state.hwnd;
            let _ = InvalidateRect(hwnd, None, true);
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
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        let state = match STATE.as_ref() {
            Some(s) => s,
            None => {
                EndPaint(hwnd, &ps);
                return;
            }
        };

        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).unwrap_or_default();

        // Background
        let bg_brush = CreateSolidBrush(state.accent_color);
        FillRect(hdc, &rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        // Text color: white
        SetTextColor(hdc, COLORREF(0x00FFFFFF));
        SetBkMode(hdc, TRANSPARENT);

        // Status text (top area, bold)
        let font = CreateFontW(
            20, 0, 0, 0,
            700, // bold
            0, 0, 0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            DEFAULT_PITCH.0 as u32 | FF_SWISS.0 as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(hdc, font);

        let mut status: Vec<u16> = state.status_text.encode_utf16().chain(Some(0)).collect();
        let mut status_rect = RECT {
            left: 16,
            top: 6,
            right: rect.right - 16,
            bottom: 30,
        };
        DrawTextW(hdc, &mut status, &mut status_rect, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);

        // Interim text (bottom area, smaller, regular weight)
        if !state.interim_text.is_empty() {
            let small_font = CreateFontW(
                15, 0, 0, 0,
                400, // regular
                0, 0, 0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                DEFAULT_PITCH.0 as u32 | FF_SWISS.0 as u32,
                w!("Segoe UI"),
            );
            SelectObject(hdc, small_font);

            // Show last ~50 chars of interim text
            let display_text = if state.interim_text.chars().count() > 50 {
                let start = state.interim_text.char_indices()
                    .rev()
                    .nth(49)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                format!("…{}", &state.interim_text[start..])
            } else {
                state.interim_text.clone()
            };

            let mut interim: Vec<u16> = display_text.encode_utf16().chain(Some(0)).collect();
            let mut interim_rect = RECT {
                left: 16,
                top: 32,
                right: rect.right - 16,
                bottom: rect.bottom - 4,
            };
            SetTextColor(hdc, COLORREF(0x00E0E0E0)); // slightly dimmer
            DrawTextW(hdc, &mut interim, &mut interim_rect, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);

            let _ = DeleteObject(small_font);
        }

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);

        EndPaint(hwnd, &ps);
    }
}

#[cfg(not(windows))]
mod platform {
    use super::OverlayMsg;
    use std::sync::mpsc;

    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        // Linux: TODO — for now just drain messages
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
