//! Floating status overlay — a compact pill at the bottom center of the screen.
//!
//! Renders with Direct2D for antialiased rounded corners, animated icons,
//! and smooth fade in/out via UpdateLayeredWindow per-pixel alpha.

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

    // ── Win32 windowing via windows-sys ──────────────────────
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    // ── Direct2D / DirectWrite via windows crate ────────────
    use windows::core::Interface;
    use windows::Win32::Foundation::RECT as D2RECT;
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Direct2D::*;
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Gdi::HDC as D2HDC;

    // ── Monitor & DPI APIs (not in windows-sys features) ────
    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    extern "system" {
        fn MonitorFromWindow(hwnd: HWND, dwflags: u32) -> *mut std::ffi::c_void;
        fn GetMonitorInfoW(hmonitor: *mut std::ffi::c_void, lpmi: *mut MONITORINFO) -> BOOL;
        fn SetProcessDpiAwarenessContext(value: isize) -> BOOL;
        fn GetDpiForWindow(hwnd: HWND) -> u32;
        fn UpdateLayeredWindow(
            hwnd: HWND,
            hdcdst: HDC,
            pptdst: *const POINT,
            psize: *const SIZE,
            hdcsrc: HDC,
            pptsrc: *const POINT,
            crkey: u32,
            pblend: *const BLENDFUNCTION,
            dwflags: u32,
        ) -> BOOL;
    }

    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
    const ULW_ALPHA: u32 = 2;
    const AC_SRC_OVER: u8 = 0;
    const AC_SRC_ALPHA: u8 = 1;

    #[repr(C)]
    struct BLENDFUNCTION {
        blend_op: u8,
        blend_flags: u8,
        source_constant_alpha: u8,
        alpha_format: u8,
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct MONITORINFO {
        cbSize: u32,
        rcMonitor: RECT,
        rcWork: RECT,
        dwFlags: u32,
    }

    // ── Base dimensions at 96 DPI ───────────────────────────
    const BASE_DPI: f32 = 96.0;
    const BASE_PILL_WIDTH: f32 = 280.0;
    const BASE_PILL_HEIGHT_SMALL: f32 = 36.0;
    const BASE_PILL_HEIGHT_LARGE: f32 = 54.0;
    const BASE_CORNER_RADIUS: f32 = 18.0;

    // Font
    const BASE_STATUS_FONT: f32 = 15.0;
    const BASE_INTERIM_FONT: f32 = 12.5;

    // Waveform bars
    const BAR_COUNT: usize = 5;
    const BAR_WIDTH: f32 = 3.0;
    const BAR_SPACING: f32 = 2.0;
    const BAR_MIN_H: f32 = 3.0;
    const BAR_MAX_H: f32 = 16.0;

    // Processing dots
    const DOT_COUNT: usize = 3;
    const DOT_BASE_RADIUS: f32 = 2.5;
    const DOT_SPACING: f32 = 8.0;

    // Timing
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 33; // ~30fps

    // Fade
    const FADE_IN_STEPS: u8 = 7;  // ~230ms at 33ms/tick
    const FADE_OUT_STEPS: u8 = 9; // ~300ms at 33ms/tick
    const MAX_ALPHA: u8 = 230;

    // Background color (RGBA)
    const BG_R: f32 = 0.11;
    const BG_G: f32 = 0.10;
    const BG_B: f32 = 0.10;
    const BG_A: f32 = 0.88;

    // ── Overlay mode ────────────────────────────────────────
    #[derive(Clone, Copy, PartialEq)]
    enum OverlayMode {
        None,
        Waveform,   // recording — animated bars
        Processing, // connecting / recognizing / thinking — bouncing dots
        Success,    // done — animated checkmark
        Error,      // error — X mark
    }

    // ── State ───────────────────────────────────────────────
    struct OverlayState {
        status_text: String,
        interim_text: String,
        accent_color: D2D1_COLOR_F,
        mode: OverlayMode,
        visible: bool,
        hwnd: HWND,
        dismiss_at: Option<std::time::Instant>,

        // D2D resources — dc_target for BindDC, render_target for drawing
        dc_target: ID2D1DCRenderTarget,
        render_target: ID2D1RenderTarget,
        dwrite_factory: IDWriteFactory,

        // Animation
        tick: u32,
        alpha: u8,
        target_alpha: u8,

        // Current geometry (physical pixels)
        pill_x: i32,
        pill_y: i32,
        pill_w: i32,
        pill_h: i32,
        dpi_scale: f32,
    }

    /// Single-thread cell usable as a `static`. Only accessed from the overlay thread.
    struct ThreadLocal<T>(std::cell::UnsafeCell<T>);
    unsafe impl<T> Sync for ThreadLocal<T> {}
    impl<T> ThreadLocal<T> {
        const fn new(val: T) -> Self {
            Self(std::cell::UnsafeCell::new(val))
        }
        unsafe fn get(&self) -> &mut T {
            &mut *self.0.get()
        }
    }

    static STATE: ThreadLocal<Option<OverlayState>> = ThreadLocal::new(None);
    static RX: ThreadLocal<Option<mpsc::Receiver<OverlayMsg>>> = ThreadLocal::new(None);

    // ── Status mapping ──────────────────────────────────────
    fn status_for_state(state: &str) -> (&str, OverlayMode, D2D1_COLOR_F) {
        let color = |r: f32, g: f32, b: f32| D2D1_COLOR_F {
            r,
            g,
            b,
            a: 1.0,
        };
        match state {
            s if s.starts_with("recording") => {
                ("Listening...", OverlayMode::Waveform, color(1.0, 0.25, 0.25))
            }
            "connecting_asr" => (
                "Connecting...",
                OverlayMode::Processing,
                color(1.0, 0.8, 0.25),
            ),
            "finalizing_asr" => (
                "Recognizing...",
                OverlayMode::Processing,
                color(0.25, 0.8, 1.0),
            ),
            "correcting" => (
                "Thinking...",
                OverlayMode::Processing,
                color(0.38, 0.56, 1.0),
            ),
            s if s.starts_with("preparing_paste") || s == "pasting" => {
                ("Done!", OverlayMode::Success, color(0.38, 0.87, 0.38))
            }
            "failed" | "error" => ("Error", OverlayMode::Error, color(1.0, 0.25, 0.25)),
            _ => ("", OverlayMode::None, color(0.5, 0.5, 0.5)),
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe fn get_dpi_scale(hwnd: HWND) -> f32 {
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 {
            1.0
        } else {
            dpi as f32 / BASE_DPI
        }
    }

    fn sc(base: f32, scale: f32) -> f32 {
        base * scale
    }

    fn sci(base: f32, scale: f32) -> i32 {
        (base * scale).round() as i32
    }

    // ── D2D initialization ──────────────────────────────────
    fn init_d2d(
    ) -> windows::core::Result<(ID2D1DCRenderTarget, ID2D1RenderTarget, IDWriteFactory)> {
        unsafe {
            let d2d_factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;

            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let dc_target: ID2D1DCRenderTarget = d2d_factory.CreateDCRenderTarget(&props)?;

            // Cast to ID2D1RenderTarget for drawing methods
            let render_target: ID2D1RenderTarget = dc_target.cast()?;

            let dwrite_factory: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;

            Ok((dc_target, render_target, dwrite_factory))
        }
    }

    // ── Rendering ───────────────────────────────────────────
    unsafe fn render(state: &mut OverlayState) {
        if state.alpha == 0 {
            return;
        }

        let w = state.pill_w;
        let h = state.pill_h;
        if w <= 0 || h <= 0 {
            return;
        }

        // Create memory DC + 32-bit ARGB bitmap
        let screen_dc = GetDC(std::ptr::null_mut());
        let mem_dc = CreateCompatibleDC(screen_dc);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbitmap = CreateDIBSection(
            mem_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if hbitmap.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return;
        }
        let old_bmp = SelectObject(mem_dc, hbitmap);

        // Bind D2D DC target to memory DC
        let bind_rect = D2RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        if state
            .dc_target
            .BindDC(D2HDC(mem_dc as *mut _), &bind_rect)
            .is_err()
        {
            SelectObject(mem_dc, old_bmp);
            DeleteObject(hbitmap);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return;
        }

        let target = &state.render_target;
        let dpi = state.dpi_scale;

        target.BeginDraw();
        target.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        // ── Shadow (subtle dark glow behind the pill) ──
        let shadow_expand = sc(3.0, dpi);
        let shadow_radius = sc(BASE_CORNER_RADIUS, dpi) + shadow_expand;
        let shadow_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: -shadow_expand,
                top: -shadow_expand,
                right: w as f32 + shadow_expand,
                bottom: h as f32 + shadow_expand,
            },
            radiusX: shadow_radius,
            radiusY: shadow_radius,
        };
        if let Ok(shadow_brush) = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.18,
            },
            None,
        ) {
            target.FillRoundedRectangle(&shadow_rect, &shadow_brush);
        }

        // ── Background rounded pill ──
        let corner = sc(BASE_CORNER_RADIUS, dpi);
        let bg_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w as f32,
                bottom: h as f32,
            },
            radiusX: corner,
            radiusY: corner,
        };
        if let Ok(bg_brush) = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: BG_R,
                g: BG_G,
                b: BG_B,
                a: BG_A,
            },
            None,
        ) {
            target.FillRoundedRectangle(&bg_rect, &bg_brush);
        }

        // ── Subtle border ──
        if let Ok(border_brush) = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.08,
            },
            None,
        ) {
            let inset = 0.5;
            let border_rect = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: inset,
                    top: inset,
                    right: w as f32 - inset,
                    bottom: h as f32 - inset,
                },
                radiusX: corner - inset,
                radiusY: corner - inset,
            };
            target.DrawRoundedRectangle(&border_rect, &border_brush, 1.0, None);
        }

        // ── Icon area ──
        let icon_cx = sc(20.0, dpi);
        let icon_cy = if state.interim_text.is_empty() {
            h as f32 / 2.0
        } else {
            sc(18.0, dpi)
        };

        draw_icon(target, state, icon_cx, icon_cy, dpi);

        // ── Text ──
        draw_text(target, state, w, h, dpi);

        let _ = target.EndDraw(None, None);

        // ── UpdateLayeredWindow ──
        let dst_point = POINT {
            x: state.pill_x,
            y: state.pill_y,
        };
        let src_point = POINT { x: 0, y: 0 };
        let win_size = SIZE { cx: w, cy: h };
        let blend = BLENDFUNCTION {
            blend_op: AC_SRC_OVER,
            blend_flags: 0,
            source_constant_alpha: state.alpha,
            alpha_format: AC_SRC_ALPHA,
        };

        UpdateLayeredWindow(
            state.hwnd,
            screen_dc,
            &dst_point,
            &win_size,
            mem_dc,
            &src_point,
            0,
            &blend,
            ULW_ALPHA,
        );

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        DeleteObject(hbitmap);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
    }

    // ── Icon drawing ────────────────────────────────────────
    unsafe fn draw_icon(
        target: &ID2D1RenderTarget,
        state: &OverlayState,
        cx: f32,
        cy: f32,
        dpi: f32,
    ) {
        match state.mode {
            OverlayMode::Waveform => {
                draw_waveform(target, &state.accent_color, state.tick, cx, cy, dpi)
            }
            OverlayMode::Processing => {
                draw_dots(target, &state.accent_color, state.tick, cx, cy, dpi)
            }
            OverlayMode::Success => {
                draw_checkmark(target, &state.accent_color, state.tick, cx, cy, dpi)
            }
            OverlayMode::Error => draw_cross(target, &state.accent_color, cx, cy, dpi),
            OverlayMode::None => {}
        }
    }

    unsafe fn draw_waveform(
        target: &ID2D1RenderTarget,
        color: &D2D1_COLOR_F,
        tick: u32,
        cx: f32,
        cy: f32,
        dpi: f32,
    ) {
        let bar_w = sc(BAR_WIDTH, dpi);
        let total_w = BAR_COUNT as f32 * bar_w + (BAR_COUNT as f32 - 1.0) * sc(BAR_SPACING, dpi);
        let start_x = cx - total_w / 2.0;

        for i in 0..BAR_COUNT {
            let phase = tick as f64 * 0.12 + i as f64 * 1.1;
            let t = (0.5 + 0.5 * phase.sin()) as f32;
            let h = sc(BAR_MIN_H + t * (BAR_MAX_H - BAR_MIN_H), dpi);
            let alpha = 0.55 + 0.45 * t;

            let bar_color = D2D1_COLOR_F {
                r: color.r,
                g: color.g,
                b: color.b,
                a: alpha,
            };
            if let Ok(brush) = target.CreateSolidColorBrush(&bar_color, None) {
                let x = start_x + i as f32 * (bar_w + sc(BAR_SPACING, dpi));
                let y = cy - h / 2.0;
                let rounded = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: x,
                        top: y,
                        right: x + bar_w,
                        bottom: y + h,
                    },
                    radiusX: bar_w / 2.0,
                    radiusY: bar_w / 2.0,
                };
                target.FillRoundedRectangle(&rounded, &brush);
            }
        }
    }

    unsafe fn draw_dots(
        target: &ID2D1RenderTarget,
        color: &D2D1_COLOR_F,
        tick: u32,
        cx: f32,
        cy: f32,
        dpi: f32,
    ) {
        let total_w = (DOT_COUNT as f32 - 1.0) * sc(DOT_SPACING, dpi);
        let start_x = cx - total_w / 2.0;

        for i in 0..DOT_COUNT {
            let phase = tick as f64 * 0.15 - i as f64 * 0.9;
            let bounce = phase.sin().max(0.0) as f32;
            let r = sc(DOT_BASE_RADIUS + bounce * 1.5, dpi);
            let alpha = 0.35 + 0.65 * bounce;
            let offset_y = bounce * sc(3.0, dpi);

            let dot_color = D2D1_COLOR_F {
                r: color.r,
                g: color.g,
                b: color.b,
                a: alpha,
            };
            if let Ok(brush) = target.CreateSolidColorBrush(&dot_color, None) {
                let x = start_x + i as f32 * sc(DOT_SPACING, dpi);
                let ellipse = D2D1_ELLIPSE {
                    point: D2D_POINT_2F {
                        x,
                        y: cy - offset_y,
                    },
                    radiusX: r,
                    radiusY: r,
                };
                target.FillEllipse(&ellipse, &brush);
            }
        }
    }

    unsafe fn draw_checkmark(
        target: &ID2D1RenderTarget,
        color: &D2D1_COLOR_F,
        tick: u32,
        cx: f32,
        cy: f32,
        dpi: f32,
    ) {
        let progress = (tick as f32 / 12.0).min(1.0);

        // Checkmark ✓ — in D2D coords Y increases downward:
        // left-mid → down to bottom → up to top-right
        let p0 = D2D_POINT_2F {
            x: cx - sc(6.0, dpi),
            y: cy - sc(1.0, dpi),
        };
        let p1 = D2D_POINT_2F {
            x: cx - sc(1.5, dpi),
            y: cy + sc(4.0, dpi),
        };
        let p2 = D2D_POINT_2F {
            x: cx + sc(7.0, dpi),
            y: cy - sc(5.0, dpi),
        };

        let stroke_color = D2D1_COLOR_F {
            r: color.r,
            g: color.g,
            b: color.b,
            a: 0.95,
        };
        if let Ok(brush) = target.CreateSolidColorBrush(&stroke_color, None) {
            let stroke_w = sc(2.0, dpi);
            let factory = target.GetFactory().expect("D2D factory");
            let style = create_round_stroke_style(&factory);

            if progress <= 0.4 {
                let t = progress / 0.4;
                let end = D2D_POINT_2F {
                    x: p0.x + (p1.x - p0.x) * t,
                    y: p0.y + (p1.y - p0.y) * t,
                };
                target.DrawLine(p0, end, &brush, stroke_w, style.as_ref());
            } else {
                let t = (progress - 0.4) / 0.6;
                let end = D2D_POINT_2F {
                    x: p1.x + (p2.x - p1.x) * t,
                    y: p1.y + (p2.y - p1.y) * t,
                };
                target.DrawLine(p0, p1, &brush, stroke_w, style.as_ref());
                target.DrawLine(p1, end, &brush, stroke_w, style.as_ref());
            }
        }
    }

    unsafe fn draw_cross(
        target: &ID2D1RenderTarget,
        color: &D2D1_COLOR_F,
        cx: f32,
        cy: f32,
        dpi: f32,
    ) {
        let arm = sc(5.0, dpi);
        let stroke_color = D2D1_COLOR_F {
            r: color.r,
            g: color.g,
            b: color.b,
            a: 0.95,
        };
        if let Ok(brush) = target.CreateSolidColorBrush(&stroke_color, None) {
            let stroke_w = sc(2.0, dpi);
            let factory = target.GetFactory().expect("D2D factory");
            let style = create_round_stroke_style(&factory);

            target.DrawLine(
                D2D_POINT_2F {
                    x: cx - arm,
                    y: cy - arm,
                },
                D2D_POINT_2F {
                    x: cx + arm,
                    y: cy + arm,
                },
                &brush,
                stroke_w,
                style.as_ref(),
            );
            target.DrawLine(
                D2D_POINT_2F {
                    x: cx + arm,
                    y: cy - arm,
                },
                D2D_POINT_2F {
                    x: cx - arm,
                    y: cy + arm,
                },
                &brush,
                stroke_w,
                style.as_ref(),
            );
        }
    }

    unsafe fn create_round_stroke_style(factory: &ID2D1Factory) -> Option<ID2D1StrokeStyle> {
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 1.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        factory.CreateStrokeStyle(&props, None).ok()
    }

    // ── Text drawing ────────────────────────────────────────
    unsafe fn draw_text(
        target: &ID2D1RenderTarget,
        state: &OverlayState,
        w: i32,
        h: i32,
        dpi: f32,
    ) {
        let text_left = sc(36.0, dpi);
        let pad_right = sc(14.0, dpi);
        let font_name_wide = to_wide("Segoe UI");
        let locale_wide = to_wide("en-us");

        // Status text
        if let Ok(fmt) = state.dwrite_factory.CreateTextFormat(
            windows::core::PCWSTR(font_name_wide.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            sc(BASE_STATUS_FONT, dpi),
            windows::core::PCWSTR(locale_wide.as_ptr()),
        ) {
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            let text_color = D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.92,
            };
            if let Ok(brush) = target.CreateSolidColorBrush(&text_color, None) {
                let status_wide = to_wide(&state.status_text);
                let top = if state.interim_text.is_empty() {
                    0.0
                } else {
                    sc(4.0, dpi)
                };
                let bottom = if state.interim_text.is_empty() {
                    h as f32
                } else {
                    sc(24.0, dpi)
                };
                let rect = D2D_RECT_F {
                    left: text_left,
                    top,
                    right: w as f32 - pad_right,
                    bottom,
                };
                target.DrawText(
                    &status_wide[..status_wide.len() - 1], // exclude null terminator
                    &fmt,
                    &rect,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }

        // Interim text
        if !state.interim_text.is_empty() {
            if let Ok(fmt) = state.dwrite_factory.CreateTextFormat(
                windows::core::PCWSTR(font_name_wide.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_REGULAR,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                sc(BASE_INTERIM_FONT, dpi),
                windows::core::PCWSTR(locale_wide.as_ptr()),
            ) {
                let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = fmt.SetTrimming(
                    &DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    },
                    None,
                );

                let text_color = D2D1_COLOR_F {
                    r: 0.69,
                    g: 0.69,
                    b: 0.69,
                    a: 1.0,
                };
                if let Ok(brush) = target.CreateSolidColorBrush(&text_color, None) {
                    // Show tail of long text
                    let display_text = if state.interim_text.chars().count() > 40 {
                        let start = state
                            .interim_text
                            .char_indices()
                            .rev()
                            .nth(39)
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        format!("\u{2026}{}", &state.interim_text[start..])
                    } else {
                        state.interim_text.clone()
                    };

                    let interim_wide = to_wide(&display_text);
                    let rect = D2D_RECT_F {
                        left: text_left,
                        top: sc(28.0, dpi),
                        right: w as f32 - pad_right,
                        bottom: h as f32 - sc(4.0, dpi),
                    };
                    target.DrawText(
                        &interim_wide[..interim_wide.len() - 1],
                        &fmt,
                        &rect,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        }
    }

    // ── Window management ───────────────────────────────────
    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

            *RX.get() = Some(rx);

            // Initialize D2D
            let (dc_target, render_target, dwrite_factory) = match init_d2d() {
                Ok(r) => r,
                Err(e) => {
                    log::error!("overlay: D2D init failed: {e}");
                    return;
                }
            };

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

            let window_name = to_wide("Koe");
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_LAYERED
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );

            let dpi_val = get_dpi_scale(hwnd);

            *STATE.get() = Some(OverlayState {
                status_text: String::new(),
                interim_text: String::new(),
                accent_color: D2D1_COLOR_F {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                },
                mode: OverlayMode::None,
                visible: false,
                hwnd,
                dismiss_at: None,
                dc_target,
                render_target,
                dwrite_factory,
                tick: 0,
                alpha: 0,
                target_alpha: 0,
                pill_x: 0,
                pill_y: 0,
                pill_w: sci(BASE_PILL_WIDTH, dpi_val),
                pill_h: sci(BASE_PILL_HEIGHT_SMALL, dpi_val),
                dpi_scale: dpi_val,
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
            WM_TIMER if wparam == TIMER_ID as WPARAM => {
                poll_messages();
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
        let rx = match RX.get().as_ref() {
            Some(r) => r,
            None => return,
        };

        let mut needs_render = false;

        while let Ok(msg) = rx.try_recv() {
            let state = STATE.get().as_mut().unwrap();
            match msg {
                OverlayMsg::UpdateState(s_str) => {
                    let (text, mode, color) = status_for_state(&s_str);

                    if s_str == "idle" || s_str == "completed" {
                        if !state.visible {
                            continue;
                        }
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(600),
                        );
                    } else if s_str.starts_with("preparing_paste") || s_str == "pasting" {
                        state.dismiss_at = Some(
                            std::time::Instant::now() + std::time::Duration::from_millis(800),
                        );
                        state.status_text = text.to_string();
                        state.accent_color = color;
                        state.mode = mode;
                        state.tick = 0;
                        state.interim_text.clear();
                        reposition_pill(state);
                        show_window(state);
                    } else {
                        state.dismiss_at = None;
                        state.status_text = text.to_string();
                        state.accent_color = color;
                        state.mode = mode;
                        if !text.is_empty() {
                            reposition_pill(state);
                            show_window(state);
                        }
                    }
                    needs_render = true;
                }
                OverlayMsg::UpdateInterimText(text) => {
                    let state = STATE.get().as_mut().unwrap();
                    let had_interim = !state.interim_text.is_empty();
                    state.interim_text = text;
                    let has_interim = !state.interim_text.is_empty();
                    if had_interim != has_interim {
                        reposition_pill(state);
                    }
                    needs_render = true;
                }
                OverlayMsg::Dismiss => {
                    hide_window(STATE.get().as_mut().unwrap());
                    needs_render = false;
                }
            }
        }

        let state = STATE.get().as_mut().unwrap();

        // Check dismiss timer
        if let Some(dismiss_at) = state.dismiss_at {
            if std::time::Instant::now() >= dismiss_at {
                state.dismiss_at = None;
                state.interim_text.clear();
                hide_window(state);
                needs_render = true;
            }
        }

        // Advance animation tick
        if state.visible || state.alpha > 0 {
            state.tick = state.tick.wrapping_add(1);
            needs_render = true;
        }

        // Fade animation
        if state.alpha != state.target_alpha {
            if state.alpha < state.target_alpha {
                let step = MAX_ALPHA / FADE_IN_STEPS;
                state.alpha = state.alpha.saturating_add(step).min(state.target_alpha);
            } else {
                let step = MAX_ALPHA / FADE_OUT_STEPS;
                state.alpha = state.alpha.saturating_sub(step).max(state.target_alpha);
            }
            needs_render = true;

            // When fully faded out, actually hide the window
            if state.alpha == 0 && state.target_alpha == 0 {
                ShowWindow(state.hwnd, SW_HIDE);
                state.visible = false;
                state.status_text.clear();
                state.interim_text.clear();
            }
        }

        if needs_render {
            render(state);
        }
    }

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
            info.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }

    unsafe fn reposition_pill(state: &mut OverlayState) {
        state.dpi_scale = get_dpi_scale(state.hwnd);
        let dpi = state.dpi_scale;

        let w = sci(BASE_PILL_WIDTH, dpi);
        let h = if state.interim_text.is_empty() {
            sci(BASE_PILL_HEIGHT_SMALL, dpi)
        } else {
            sci(BASE_PILL_HEIGHT_LARGE, dpi)
        };

        let mon = active_monitor_rect();
        let mon_w = mon.right - mon.left;
        let x = mon.left + (mon_w - w) / 2;
        let y = mon.bottom - h - sci(40.0, dpi);

        state.pill_x = x;
        state.pill_y = y;
        state.pill_w = w;
        state.pill_h = h;

        SetWindowPos(
            state.hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    unsafe fn show_window(state: &mut OverlayState) {
        if !state.visible {
            ShowWindow(state.hwnd, SW_SHOWNOACTIVATE);
            state.visible = true;
        }
        state.target_alpha = MAX_ALPHA;
    }

    unsafe fn hide_window(state: &mut OverlayState) {
        // Start fade out; actual SW_HIDE happens when alpha reaches 0
        state.target_alpha = 0;
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
