//! Floating status overlay — a compact pill at the bottom center of the screen.
//!
//! Renders with Direct2D for antialiased rounded corners, animated icons,
//! and smooth fade in/out via UpdateLayeredWindow per-pixel alpha.

use std::sync::mpsc;
use std::thread;

pub enum OverlayMsg {
    UpdateState(String),
    UpdateInterimText(String),
    /// Display text is the LLM-corrected final text shown during pasting phase.
    UpdateDisplayText(String),
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
pub fn update_display_text(text: &str) {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::UpdateDisplayText(text.to_string()));
    }
}

#[allow(dead_code)]
pub fn dismiss() {
    if let Some(tx) = TX.lock().unwrap().as_ref() {
        let _ = tx.send(OverlayMsg::Dismiss);
    }
}

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

    // ── Monitor & DPI APIs ──────────────────────────────────
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
    const BASE_MIN_PILL_WIDTH: f32 = 180.0;
    const BASE_MAX_PILL_WIDTH: f32 = 600.0;
    const BASE_PILL_HEIGHT_SMALL: f32 = 36.0;
    const BASE_CORNER_RADIUS: f32 = 18.0;
    const BASE_SCREEN_H_MARGIN: f32 = 32.0;
    const BASE_BOTTOM_MARGIN: f32 = 40.0;
    const MAX_VISIBLE_LINES: u32 = 3;

    // Font
    const BASE_STATUS_FONT: f32 = 15.0;
    const BASE_INTERIM_FONT: f32 = 15.0;

    // Icon area
    const BASE_TEXT_LEFT: f32 = 40.0;
    const BASE_PAD_RIGHT: f32 = 14.0;

    // Waveform bars
    const BAR_COUNT: usize = 5;
    const BAR_WIDTH: f32 = 3.0;
    const BAR_SPACING: f32 = 2.0;
    const BAR_MIN_H: f32 = 3.0;
    const BAR_MAX_H: f32 = 20.0;

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

    // Enter/exit position animation
    const ENTRANCE_LIFT: f32 = 10.0; // pixels to rise on show
    const EXIT_DROP: f32 = 8.0;      // pixels to drop on hide

    // Linger
    const MIN_LINGER_MS: u64 = 450;
    const MAX_LINGER_MS: u64 = 1200;
    const LINGER_MS_PER_CHAR: u64 = 15;

    // Ripple
    const RIPPLE_DURATION_TICKS: u32 = 13; // ~430ms at 33ms/tick

    // Background color (RGBA)
    const BG_R: f32 = 0.11;
    const BG_G: f32 = 0.10;
    const BG_B: f32 = 0.10;
    const BG_A: f32 = 0.88;

    // Diff animation
    const DIFF_HIGHLIGHT_TICKS: u32 = 24;  // ~800ms
    const DIFF_FADE_STEPS: u32 = 8;
    const DIFF_MAX_CHARS: usize = 500;

    // ── Overlay mode ────────────────────────────────────────
    #[derive(Clone, Copy, PartialEq)]
    enum OverlayMode {
        None,
        Waveform,   // recording — animated bars
        Processing, // connecting / recognizing / thinking — bouncing dots
        Success,    // done — animated checkmark
        Error,      // error — X mark
    }

    // ── Diff types ──────────────────────────────────────────
    #[derive(Clone, Copy, PartialEq)]
    enum DiffOp { Equal, Delete, Insert, Replace }

    #[derive(Clone)]
    struct DiffEntry { op: DiffOp, text: String }

    // ── State ───────────────────────────────────────────────
    struct OverlayState {
        status_text: String,
        interim_text: String,
        accent_color: D2D1_COLOR_F,
        mode: OverlayMode,
        visible: bool,
        hwnd: HWND,
        dismiss_at: Option<std::time::Instant>,

        // D2D resources
        dc_target: ID2D1DCRenderTarget,
        render_target: ID2D1RenderTarget,
        dwrite_factory: IDWriteFactory,

        // Animation
        tick: u32,
        alpha: u8,
        target_alpha: u8,
        audio_level: f32, // 0.0–1.0, smoothed

        // Enter/exit Y offset animation
        y_offset: f32,       // current offset in pixels (positive = lower)
        y_offset_target: f32,

        // Current geometry (physical pixels)
        pill_x: i32,
        pill_y: i32,     // base Y (without animation offset)
        pill_w: i32,
        pill_h: i32,
        dpi_scale: f32,

        // Dynamic width — session-monotonic
        session_max_w: f32,

        // Text accumulation across ASR segments
        committed_text: String,  // finalized segments concatenated
        current_segment: String, // current ASR segment interim

        // Recording ripple
        ripple_tick: Option<u32>,

        // Diff animation
        diff_entries: Option<Vec<DiffEntry>>,
        diff_step: u32,
        diff_total_steps: u32,
        diff_final_text: String,
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
        let color = |r: f32, g: f32, b: f32| D2D1_COLOR_F { r, g, b, a: 1.0 };
        match state {
            s if s.starts_with("recording") => {
                ("Listening\u{2026}", OverlayMode::Waveform, color(1.0, 0.32, 0.32))
            }
            s if s.starts_with("connecting_asr") => (
                "Connecting\u{2026}", OverlayMode::Processing, color(1.0, 0.78, 0.28),
            ),
            s if s.starts_with("finalizing_asr") => (
                "Recognizing\u{2026}", OverlayMode::Processing, color(0.35, 0.78, 1.0),
            ),
            "correcting" => (
                "Thinking\u{2026}", OverlayMode::Processing, color(0.55, 0.6, 1.0),
            ),
            s if s.starts_with("preparing_paste") || s == "pasting" => {
                ("Pasting\u{2026}", OverlayMode::Success, color(0.3, 0.85, 0.45))
            }
            "failed" | "error" => ("Error", OverlayMode::Error, color(1.0, 0.32, 0.32)),
            _ => ("Working\u{2026}", OverlayMode::Processing, color(0.35, 0.78, 1.0)),
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe fn get_dpi_scale(hwnd: HWND) -> f32 {
        let dpi = GetDpiForWindow(hwnd);
        if dpi == 0 { 1.0 } else { dpi as f32 / BASE_DPI }
    }

    fn sc(base: f32, scale: f32) -> f32 { base * scale }
    fn sci(base: f32, scale: f32) -> i32 { (base * scale).round() as i32 }

    // ── Text measurement with DirectWrite ───────────────────
    unsafe fn measure_text_width(
        dwrite: &IDWriteFactory, text: &str, font_size: f32, bold: bool, dpi: f32,
    ) -> f32 {
        let font_name = to_wide("Segoe UI");
        let locale = to_wide("en-us");
        let weight = if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_REGULAR };
        let fmt = match dwrite.CreateTextFormat(
            windows::core::PCWSTR(font_name.as_ptr()), None, weight,
            DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
            sc(font_size, dpi), windows::core::PCWSTR(locale.as_ptr()),
        ) {
            Ok(f) => f,
            Err(_) => return 0.0,
        };
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let wide = to_wide(text);
        let layout = match dwrite.CreateTextLayout(
            &wide[..wide.len() - 1], &fmt, 10000.0, 1000.0,
        ) {
            Ok(l) => l,
            Err(_) => return 0.0,
        };
        let mut metrics = std::mem::zeroed::<DWRITE_TEXT_METRICS>();
        if layout.GetMetrics(&mut metrics).is_ok() {
            metrics.width.ceil()
        } else {
            0.0
        }
    }

    /// Measure wrapped text height given a max width, and return (total_height, line_height).
    unsafe fn measure_text_height(
        dwrite: &IDWriteFactory, text: &str, font_size: f32, max_width: f32, dpi: f32,
    ) -> (f32, f32) {
        let font_name = to_wide("Segoe UI");
        let locale = to_wide("en-us");
        let fmt = match dwrite.CreateTextFormat(
            windows::core::PCWSTR(font_name.as_ptr()), None,
            DWRITE_FONT_WEIGHT_REGULAR, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
            sc(font_size, dpi), windows::core::PCWSTR(locale.as_ptr()),
        ) {
            Ok(f) => f,
            Err(_) => return (sc(font_size, dpi), sc(font_size, dpi)),
        };
        // Word-wrap
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
        let wide = to_wide(text);
        let layout = match dwrite.CreateTextLayout(
            &wide[..wide.len() - 1], &fmt, max_width.max(1.0), 100000.0,
        ) {
            Ok(l) => l,
            Err(_) => return (sc(font_size, dpi), sc(font_size, dpi)),
        };
        let mut metrics = std::mem::zeroed::<DWRITE_TEXT_METRICS>();
        if layout.GetMetrics(&mut metrics).is_ok() {
            let line_h = if metrics.lineCount > 0 {
                metrics.height / metrics.lineCount as f32
            } else {
                sc(font_size, dpi)
            };
            (metrics.height.ceil(), line_h.ceil())
        } else {
            (sc(font_size, dpi), sc(font_size, dpi))
        }
    }

    // ── Diff algorithm (character-level LCS) ────────────────
    fn compute_char_diff(old: &str, new: &str) -> Vec<DiffEntry> {
        let old_chars: Vec<char> = old.chars().collect();
        let new_chars: Vec<char> = new.chars().collect();
        let m = old_chars.len();
        let n = new_chars.len();

        // LCS DP
        let mut dp = vec![0i16; (m + 1) * (n + 1)];
        for i in 1..=m {
            for j in 1..=n {
                dp[i * (n + 1) + j] = if old_chars[i - 1] == new_chars[j - 1] {
                    dp[(i - 1) * (n + 1) + (j - 1)] + 1
                } else {
                    dp[(i - 1) * (n + 1) + j].max(dp[i * (n + 1) + (j - 1)])
                };
            }
        }

        // Backtrack
        let mut raw: Vec<(DiffOp, char)> = Vec::new();
        let (mut i, mut j) = (m, n);
        while i > 0 || j > 0 {
            if i > 0 && j > 0 && old_chars[i - 1] == new_chars[j - 1] {
                raw.push((DiffOp::Equal, old_chars[i - 1]));
                i -= 1; j -= 1;
            } else if j > 0 && (i == 0 || dp[i * (n + 1) + (j - 1)] >= dp[(i - 1) * (n + 1) + j]) {
                raw.push((DiffOp::Insert, new_chars[j - 1]));
                j -= 1;
            } else {
                raw.push((DiffOp::Delete, old_chars[i - 1]));
                i -= 1;
            }
        }
        raw.reverse();

        // Merge consecutive same-op chars
        let mut merged: Vec<DiffEntry> = Vec::new();
        for (op, ch) in raw {
            if let Some(last) = merged.last_mut() {
                if last.op == op { last.text.push(ch); continue; }
            }
            merged.push(DiffEntry { op, text: ch.to_string() });
        }

        // Merge adjacent Delete+Insert → Replace
        let mut result: Vec<DiffEntry> = Vec::new();
        let mut idx = 0;
        while idx < merged.len() {
            if merged[idx].op == DiffOp::Delete && idx + 1 < merged.len() && merged[idx + 1].op == DiffOp::Insert {
                result.push(DiffEntry { op: DiffOp::Replace, text: merged[idx + 1].text.clone() });
                idx += 2;
            } else {
                result.push(merged[idx].clone());
                idx += 1;
            }
        }
        result
    }

    // ── D2D initialization ──────────────────────────────────
    fn init_d2d() -> windows::core::Result<(ID2D1DCRenderTarget, ID2D1RenderTarget, IDWriteFactory)> {
        unsafe {
            let d2d_factory: ID2D1Factory =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 0.0, dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let dc_target: ID2D1DCRenderTarget = d2d_factory.CreateDCRenderTarget(&props)?;
            let render_target: ID2D1RenderTarget = dc_target.cast()?;
            let dwrite_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok((dc_target, render_target, dwrite_factory))
        }
    }

    // ── Rendering ───────────────────────────────────────────
    unsafe fn render(state: &mut OverlayState) {
        if state.alpha == 0 { return; }
        let w = state.pill_w;
        let h = state.pill_h;
        if w <= 0 || h <= 0 { return; }

        let screen_dc = GetDC(std::ptr::null_mut());
        let mem_dc = CreateCompatibleDC(screen_dc);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbitmap = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if hbitmap.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return;
        }
        let old_bmp = SelectObject(mem_dc, hbitmap);

        let bind_rect = D2RECT { left: 0, top: 0, right: w, bottom: h };
        if state.dc_target.BindDC(D2HDC(mem_dc as *mut _), &bind_rect).is_err() {
            SelectObject(mem_dc, old_bmp);
            DeleteObject(hbitmap);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return;
        }

        let target = &state.render_target;
        let dpi = state.dpi_scale;

        target.BeginDraw();
        target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

        // ── Shadow ──
        let shadow_expand = sc(3.0, dpi);
        let shadow_radius = sc(BASE_CORNER_RADIUS, dpi) + shadow_expand;
        if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.18 }, None) {
            target.FillRoundedRectangle(&D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: -shadow_expand, top: -shadow_expand, right: w as f32 + shadow_expand, bottom: h as f32 + shadow_expand },
                radiusX: shadow_radius, radiusY: shadow_radius,
            }, &brush);
        }

        // ── Background ──
        let corner = sc(BASE_CORNER_RADIUS, dpi);
        if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: BG_R, g: BG_G, b: BG_B, a: BG_A }, None) {
            target.FillRoundedRectangle(&D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 },
                radiusX: corner, radiusY: corner,
            }, &brush);
        }

        // ── Border ──
        if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.08 }, None) {
            let inset = 0.5;
            target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: inset, top: inset, right: w as f32 - inset, bottom: h as f32 - inset },
                radiusX: corner - inset, radiusY: corner - inset,
            }, &brush, 1.0, None);
        }

        // ── Ripple ──
        if let Some(rt) = state.ripple_tick {
            let progress = rt as f32 / RIPPLE_DURATION_TICKS as f32;
            if progress <= 1.0 {
                let icon_cx = sc(20.0, dpi);
                let icon_cy = h as f32 / 2.0;
                let max_r = sc(18.0, dpi);
                let r = sc(6.0, dpi) + (max_r - sc(6.0, dpi)) * progress;
                let alpha = 0.22 * (1.0 - progress) * (if progress < 0.3 { progress / 0.3 } else { 1.0 });
                if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: alpha }, None) {
                    let ellipse = D2D1_ELLIPSE { point: D2D_POINT_2F { x: icon_cx, y: icon_cy }, radiusX: r, radiusY: r };
                    if let Ok(stroke_brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: alpha * 0.8 }, None) {
                        target.DrawEllipse(&ellipse, &stroke_brush, sc(1.3, dpi), None);
                    }
                    let _ = brush;
                }
            }
        }

        // ── Icon (always vertically centered) ──
        let icon_cx = sc(20.0, dpi);
        let icon_cy = h as f32 / 2.0;
        draw_icon(target, state, icon_cx, icon_cy, dpi);

        // ── Text ──
        draw_text(target, state, w, h, dpi);

        let _ = target.EndDraw(None, None);

        // ── UpdateLayeredWindow with Y offset ──
        let render_y = state.pill_y + state.y_offset.round() as i32;
        let dst_point = POINT { x: state.pill_x, y: render_y };
        let src_point = POINT { x: 0, y: 0 };
        let win_size = SIZE { cx: w, cy: h };
        let blend = BLENDFUNCTION {
            blend_op: AC_SRC_OVER, blend_flags: 0,
            source_constant_alpha: state.alpha, alpha_format: AC_SRC_ALPHA,
        };
        UpdateLayeredWindow(state.hwnd, screen_dc, &dst_point, &win_size, mem_dc, &src_point, 0, &blend, ULW_ALPHA);

        SelectObject(mem_dc, old_bmp);
        DeleteObject(hbitmap);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
    }

    // ── Icon drawing ────────────────────────────────────────
    unsafe fn draw_icon(target: &ID2D1RenderTarget, state: &OverlayState, cx: f32, cy: f32, dpi: f32) {
        match state.mode {
            OverlayMode::Waveform => draw_waveform(target, &state.accent_color, state.tick, state.audio_level, cx, cy, dpi),
            OverlayMode::Processing => draw_dots(target, &state.accent_color, state.tick, cx, cy, dpi),
            OverlayMode::Success => draw_checkmark(target, &state.accent_color, state.tick, cx, cy, dpi),
            OverlayMode::Error => draw_cross(target, &state.accent_color, cx, cy, dpi),
            OverlayMode::None => {}
        }
    }

    unsafe fn draw_waveform(target: &ID2D1RenderTarget, color: &D2D1_COLOR_F, tick: u32, level: f32, cx: f32, cy: f32, dpi: f32) {
        let bar_w = sc(BAR_WIDTH, dpi);
        let total_w = BAR_COUNT as f32 * bar_w + (BAR_COUNT as f32 - 1.0) * sc(BAR_SPACING, dpi);
        let start_x = cx - total_w / 2.0;
        for i in 0..BAR_COUNT {
            // Each bar has a sin phase offset for organic movement
            let phase = tick as f64 * 0.12 + i as f64 * 1.1;
            let wave = (0.5 + 0.5 * phase.sin()) as f32; // 0..1 oscillation
            // Mix audio level with subtle idle animation
            let idle = 0.08; // minimum movement when silent
            let t = idle + (1.0 - idle) * level * (0.4 + 0.6 * wave);
            let h = sc(BAR_MIN_H + t * (BAR_MAX_H - BAR_MIN_H), dpi);
            let alpha = 0.45 + 0.55 * t;
            if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: color.r, g: color.g, b: color.b, a: alpha }, None) {
                let x = start_x + i as f32 * (bar_w + sc(BAR_SPACING, dpi));
                target.FillRoundedRectangle(&D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: x, top: cy - h / 2.0, right: x + bar_w, bottom: cy + h / 2.0 },
                    radiusX: bar_w / 2.0, radiusY: bar_w / 2.0,
                }, &brush);
            }
        }
    }

    unsafe fn draw_dots(target: &ID2D1RenderTarget, color: &D2D1_COLOR_F, tick: u32, cx: f32, cy: f32, dpi: f32) {
        let total_w = (DOT_COUNT as f32 - 1.0) * sc(DOT_SPACING, dpi);
        let start_x = cx - total_w / 2.0;
        for i in 0..DOT_COUNT {
            let phase = tick as f64 * 0.15 - i as f64 * 0.9;
            let bounce = phase.sin().max(0.0) as f32;
            let r = sc(DOT_BASE_RADIUS + bounce * 1.5, dpi);
            let alpha = 0.35 + 0.65 * bounce;
            let offset_y = bounce * sc(3.0, dpi);
            if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: color.r, g: color.g, b: color.b, a: alpha }, None) {
                target.FillEllipse(&D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: start_x + i as f32 * sc(DOT_SPACING, dpi), y: cy - offset_y },
                    radiusX: r, radiusY: r,
                }, &brush);
            }
        }
    }

    unsafe fn draw_checkmark(target: &ID2D1RenderTarget, color: &D2D1_COLOR_F, tick: u32, cx: f32, cy: f32, dpi: f32) {
        let progress = (tick as f32 / 12.0).min(1.0);
        let p0 = D2D_POINT_2F { x: cx - sc(6.0, dpi), y: cy - sc(1.0, dpi) };
        let p1 = D2D_POINT_2F { x: cx - sc(1.5, dpi), y: cy + sc(4.0, dpi) };
        let p2 = D2D_POINT_2F { x: cx + sc(7.0, dpi), y: cy - sc(5.0, dpi) };
        if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: color.r, g: color.g, b: color.b, a: 0.95 }, None) {
            let stroke_w = sc(2.0, dpi);
            let factory = target.GetFactory().expect("D2D factory");
            let style = create_round_stroke_style(&factory);
            if progress <= 0.4 {
                let t = progress / 0.4;
                let end = D2D_POINT_2F { x: p0.x + (p1.x - p0.x) * t, y: p0.y + (p1.y - p0.y) * t };
                target.DrawLine(p0, end, &brush, stroke_w, style.as_ref());
            } else {
                let t = (progress - 0.4) / 0.6;
                let end = D2D_POINT_2F { x: p1.x + (p2.x - p1.x) * t, y: p1.y + (p2.y - p1.y) * t };
                target.DrawLine(p0, p1, &brush, stroke_w, style.as_ref());
                target.DrawLine(p1, end, &brush, stroke_w, style.as_ref());
            }
        }
    }

    unsafe fn draw_cross(target: &ID2D1RenderTarget, color: &D2D1_COLOR_F, cx: f32, cy: f32, dpi: f32) {
        let arm = sc(5.0, dpi);
        if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: color.r, g: color.g, b: color.b, a: 0.95 }, None) {
            let stroke_w = sc(2.0, dpi);
            let factory = target.GetFactory().expect("D2D factory");
            let style = create_round_stroke_style(&factory);
            target.DrawLine(D2D_POINT_2F { x: cx - arm, y: cy - arm }, D2D_POINT_2F { x: cx + arm, y: cy + arm }, &brush, stroke_w, style.as_ref());
            target.DrawLine(D2D_POINT_2F { x: cx + arm, y: cy - arm }, D2D_POINT_2F { x: cx - arm, y: cy + arm }, &brush, stroke_w, style.as_ref());
        }
    }

    unsafe fn create_round_stroke_style(factory: &ID2D1Factory) -> Option<ID2D1StrokeStyle> {
        factory.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND, lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 1.0, dashStyle: D2D1_DASH_STYLE_SOLID, dashOffset: 0.0,
        }, None).ok()
    }

    // ── Text drawing ────────────────────────────────────────
    unsafe fn draw_text(target: &ID2D1RenderTarget, state: &OverlayState, w: i32, h: i32, dpi: f32) {
        let text_left = sc(BASE_TEXT_LEFT, dpi);
        let pad_right = sc(BASE_PAD_RIGHT, dpi);
        let font_name_wide = to_wide("Segoe UI");
        let locale_wide = to_wide("en-us");
        let top_pad = sc(8.0, dpi);
        let bottom_pad = sc(8.0, dpi);

        if !state.interim_text.is_empty() {
            // ── Interim text only (no status text) ──
            let rect = D2D_RECT_F {
                left: text_left, top: top_pad,
                right: w as f32 - pad_right, bottom: h as f32 - bottom_pad,
            };

            if let Some(ref diff) = state.diff_entries {
                let progress = if state.diff_total_steps > 0 {
                    state.diff_step as f32 / state.diff_total_steps as f32
                } else { 0.0 };
                draw_diff_text(target, &state.dwrite_factory, diff, progress, &rect, dpi);
            } else {
                // Word-wrapped, scroll to bottom
                if let Ok(fmt) = state.dwrite_factory.CreateTextFormat(
                    windows::core::PCWSTR(font_name_wide.as_ptr()), None,
                    DWRITE_FONT_WEIGHT_REGULAR, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                    sc(BASE_INTERIM_FONT, dpi), windows::core::PCWSTR(locale_wide.as_ptr()),
                ) {
                    let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
                    if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.92 }, None) {
                        let interim_wide = to_wide(&state.interim_text);
                        let text_w = rect.right - rect.left;
                        let viewport_h = rect.bottom - rect.top;
                        if let Ok(layout) = state.dwrite_factory.CreateTextLayout(
                            &interim_wide[..interim_wide.len() - 1], &fmt, text_w.max(1.0), 100000.0,
                        ) {
                            let mut metrics = std::mem::zeroed::<DWRITE_TEXT_METRICS>();
                            let total_h = if layout.GetMetrics(&mut metrics).is_ok() {
                                metrics.height
                            } else { viewport_h };
                            let y_scroll = (total_h - viewport_h).max(0.0);
                            target.PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                            let origin = D2D_POINT_2F { x: rect.left, y: rect.top - y_scroll };
                            target.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                            target.PopAxisAlignedClip();
                        }
                    }
                }
            }
        } else {
            // ── Status text only (vertically centered) ──
            if let Ok(fmt) = state.dwrite_factory.CreateTextFormat(
                windows::core::PCWSTR(font_name_wide.as_ptr()), None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                sc(BASE_STATUS_FONT, dpi), windows::core::PCWSTR(locale_wide.as_ptr()),
            ) {
                let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                if let Ok(brush) = target.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.92 }, None) {
                    let status_wide = to_wide(&state.status_text);
                    target.DrawText(
                        &status_wide[..status_wide.len() - 1], &fmt,
                        &D2D_RECT_F { left: text_left, top: 0.0, right: w as f32 - pad_right, bottom: h as f32 },
                        &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        }
    }

    // ── Diff text rendering ─────────────────────────────────
    unsafe fn draw_diff_text(
        target: &ID2D1RenderTarget, dwrite: &IDWriteFactory,
        diff: &[DiffEntry], progress: f32, rect: &D2D_RECT_F, dpi: f32,
    ) {
        let font_name = to_wide("Segoe UI");
        let locale = to_wide("en-us");
        let fmt = match dwrite.CreateTextFormat(
            windows::core::PCWSTR(font_name.as_ptr()), None,
            DWRITE_FONT_WEIGHT_REGULAR, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
            sc(BASE_INTERIM_FONT, dpi), windows::core::PCWSTR(locale.as_ptr()),
        ) {
            Ok(f) => f,
            Err(_) => return,
        };
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);

        // Draw each segment with appropriate color
        let mut x_offset = rect.left;
        for entry in diff {
            if entry.op == DiffOp::Delete {
                // Deleted text: red, fading out
                let alpha = 0.55 * (1.0 - progress);
                if alpha < 0.01 { continue; }
                let color = D2D1_COLOR_F { r: 1.0, g: 0.62, b: 0.58, a: alpha };
                x_offset += draw_text_segment(target, dwrite, &entry.text, &fmt, &color, x_offset, rect, dpi);
            } else {
                let color = match entry.op {
                    DiffOp::Insert => D2D1_COLOR_F {
                        r: 0.68 + 0.32 * progress, g: 0.78 + 0.22 * progress,
                        b: 1.0 - 0.08 * progress, a: 0.92,
                    },
                    DiffOp::Replace => D2D1_COLOR_F {
                        r: 0.72 + 0.28 * progress, g: 0.82 + 0.18 * progress,
                        b: 0.98 - 0.06 * progress, a: 0.92,
                    },
                    _ => D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.92 }, // Equal
                };
                x_offset += draw_text_segment(target, dwrite, &entry.text, &fmt, &color, x_offset, rect, dpi);
            }
        }
    }

    unsafe fn draw_text_segment(
        target: &ID2D1RenderTarget, dwrite: &IDWriteFactory,
        text: &str, fmt: &IDWriteTextFormat, color: &D2D1_COLOR_F,
        x: f32, rect: &D2D_RECT_F, _dpi: f32,
    ) -> f32 {
        if let Ok(brush) = target.CreateSolidColorBrush(color, None) {
            let wide = to_wide(text);
            let seg_rect = D2D_RECT_F { left: x, top: rect.top, right: rect.right, bottom: rect.bottom };
            target.DrawText(
                &wide[..wide.len() - 1], fmt, &seg_rect, &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL,
            );
            // Measure width to advance cursor
            if let Ok(layout) = dwrite.CreateTextLayout(&wide[..wide.len() - 1], fmt, 10000.0, 1000.0) {
                let mut metrics = std::mem::zeroed::<DWRITE_TEXT_METRICS>();
                if layout.GetMetrics(&mut metrics).is_ok() {
                    return metrics.width;
                }
            }
        }
        0.0
    }

    // ── Window management ───────────────────────────────────
    pub fn run_overlay(rx: mpsc::Receiver<OverlayMsg>) {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            *RX.get() = Some(rx);

            let (dc_target, render_target, dwrite_factory) = match init_d2d() {
                Ok(r) => r,
                Err(e) => { log::error!("overlay: D2D init failed: {e}"); return; }
            };

            let class_name = to_wide("KoeOverlay");
            let hinstance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0, cbWndExtra: 0,
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
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                class_name.as_ptr(), window_name.as_ptr(), WS_POPUP,
                0, 0, 1, 1,
                std::ptr::null_mut(), std::ptr::null_mut(), hinstance, std::ptr::null(),
            );

            let dpi_val = get_dpi_scale(hwnd);
            *STATE.get() = Some(OverlayState {
                status_text: String::new(),
                interim_text: String::new(),
                accent_color: D2D1_COLOR_F { r: 0.5, g: 0.5, b: 0.5, a: 1.0 },
                mode: OverlayMode::None,
                visible: false, hwnd,
                dismiss_at: None,
                dc_target, render_target, dwrite_factory,
                tick: 0, alpha: 0, target_alpha: 0, audio_level: 0.0,
                y_offset: sc(ENTRANCE_LIFT, dpi_val),
                y_offset_target: 0.0,
                pill_x: 0, pill_y: 0,
                pill_w: sci(BASE_MIN_PILL_WIDTH, dpi_val),
                pill_h: sci(BASE_PILL_HEIGHT_SMALL, dpi_val),
                dpi_scale: dpi_val,
                session_max_w: 0.0,
                committed_text: String::new(),
                current_segment: String::new(),
                ripple_tick: None,
                diff_entries: None, diff_step: 0, diff_total_steps: 0,
                diff_final_text: String::new(),
            });

            SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER if wparam == TIMER_ID as WPARAM => { poll_messages(); 0 }
            WM_DESTROY => { PostQuitMessage(0); 0 }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn poll_messages() {
        let rx = match RX.get().as_ref() { Some(r) => r, None => return };
        let mut needs_render = false;

        while let Ok(msg) = rx.try_recv() {
            let state = STATE.get().as_mut().unwrap();
            match msg {
                OverlayMsg::UpdateState(s_str) => {
                    let (text, mode, color) = status_for_state(&s_str);
                    let was_recording = state.mode == OverlayMode::Waveform;

                    if s_str == "idle" || s_str == "completed" {
                        if !state.visible { continue; }
                        // Adaptive linger based on displayed text length
                        let display_len = state.interim_text.len().max(state.status_text.len());
                        let linger = (display_len as u64 * LINGER_MS_PER_CHAR)
                            .clamp(MIN_LINGER_MS, MAX_LINGER_MS);
                        state.dismiss_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(linger));
                    } else if s_str.starts_with("preparing_paste") || s_str == "pasting" {
                        let display_len = state.interim_text.len().max(state.status_text.len());
                        let linger = (display_len as u64 * LINGER_MS_PER_CHAR)
                            .clamp(MIN_LINGER_MS, MAX_LINGER_MS);
                        state.dismiss_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(linger));
                        state.status_text = text.to_string();
                        state.accent_color = color;
                        state.mode = mode;
                        state.tick = 0;
                        // Hide accumulated text, show only icon + status
                        state.interim_text.clear();
                        state.committed_text.clear();
                        state.current_segment.clear();
                        state.diff_entries = None;
                        reposition_pill(state);
                        show_window(state);
                    } else {
                        state.dismiss_at = None;
                        state.status_text = text.to_string();
                        state.accent_color = color;
                        state.mode = mode;
                        state.diff_entries = None;

                        // New recording session: reset accumulation & start ripple
                        if mode == OverlayMode::Waveform && !was_recording {
                            state.session_max_w = 0.0;
                            state.committed_text.clear();
                            state.current_segment.clear();
                            state.interim_text.clear();
                            state.ripple_tick = Some(0);
                        }

                        // Non-recording states: hide text, show only icon + status
                        if mode != OverlayMode::Waveform {
                            state.interim_text.clear();
                            state.committed_text.clear();
                            state.current_segment.clear();
                        }

                        if !text.is_empty() {
                            reposition_pill(state);
                            show_window(state);
                        }
                    }
                    needs_render = true;
                }
                OverlayMsg::UpdateInterimText(text) => {
                    let state = STATE.get().as_mut().unwrap();
                    if text.is_empty() { continue; }

                    // Detect ASR segment reset: new text is much shorter than
                    // current segment → the ASR restarted its buffer after a pause.
                    let cur_len = state.current_segment.chars().count();
                    let new_len = text.chars().count();
                    if cur_len > 3 && new_len < cur_len / 2 {
                        // Commit previous segment
                        state.committed_text.push_str(&state.current_segment);
                    }

                    state.current_segment = text;
                    // Display = all committed segments + current segment
                    state.interim_text = if state.committed_text.is_empty() {
                        state.current_segment.clone()
                    } else {
                        format!("{}{}", state.committed_text, state.current_segment)
                    };
                    state.diff_entries = None;
                    reposition_pill(state);
                    needs_render = true;
                }
                OverlayMsg::UpdateDisplayText(text) => {
                    let state = STATE.get().as_mut().unwrap();
                    let old_text = state.interim_text.clone();
                    let has_existing = !old_text.is_empty();
                    let changed = old_text != text;

                    if has_existing && changed && old_text.len() <= DIFF_MAX_CHARS && text.len() <= DIFF_MAX_CHARS {
                        // Start diff animation
                        let diff = compute_char_diff(&old_text, &text);
                        let has_diff = diff.iter().any(|e| e.op != DiffOp::Equal);
                        if has_diff {
                            // Build display text from diff (equal + insert + replace, skip delete)
                            let display: String = diff.iter().filter_map(|e| {
                                if e.op == DiffOp::Delete { None } else { Some(e.text.as_str()) }
                            }).collect();
                            state.interim_text = display;
                            state.diff_entries = Some(diff);
                            state.diff_step = 0;
                            state.diff_total_steps = DIFF_FADE_STEPS;
                            state.diff_final_text = text;
                        } else {
                            state.interim_text = text;
                            state.diff_entries = None;
                        }
                    } else {
                        state.interim_text = text;
                        state.diff_entries = None;
                    }
                    reposition_pill(state);
                    needs_render = true;
                }
                OverlayMsg::Dismiss => {
                    hide_window(STATE.get().as_mut().unwrap());
                }
            }
        }

        let state = STATE.get().as_mut().unwrap();

        // Dismiss timer
        if let Some(dismiss_at) = state.dismiss_at {
            if std::time::Instant::now() >= dismiss_at {
                state.dismiss_at = None;
                state.interim_text.clear();
                state.diff_entries = None;
                hide_window(state);
                needs_render = true;
            }
        }

        // Animation tick + audio level
        if state.visible || state.alpha > 0 {
            state.tick = state.tick.wrapping_add(1);
            // Smooth audio level (fast attack, slow decay)
            let raw = crate::audio::audio_level();
            if raw > state.audio_level {
                state.audio_level += (raw - state.audio_level) * 0.6; // fast attack
            } else {
                state.audio_level += (raw - state.audio_level) * 0.15; // slow decay
            }
            needs_render = true;
        }

        // Ripple tick
        if let Some(ref mut rt) = state.ripple_tick {
            *rt += 1;
            if *rt > RIPPLE_DURATION_TICKS {
                state.ripple_tick = None;
            }
            needs_render = true;
        }

        // Diff animation tick
        if state.diff_entries.is_some() {
            // Advance every N ticks
            let interval = DIFF_HIGHLIGHT_TICKS / DIFF_FADE_STEPS.max(1);
            if state.tick % interval == 0 && state.diff_step < state.diff_total_steps {
                state.diff_step += 1;
            }
            if state.diff_step >= state.diff_total_steps {
                // Diff animation complete — show clean final text
                if !state.diff_final_text.is_empty() {
                    state.interim_text = std::mem::take(&mut state.diff_final_text);
                }
                state.diff_entries = None;
                state.diff_step = 0;
            }
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
            if state.alpha == 0 && state.target_alpha == 0 {
                ShowWindow(state.hwnd, SW_HIDE);
                state.visible = false;
                state.status_text.clear();
                state.interim_text.clear();
                state.session_max_w = 0.0;
                state.diff_entries = None;
            }
        }

        // Y-offset animation (enter lift / exit drop)
        if (state.y_offset - state.y_offset_target).abs() > 0.5 {
            // Ease toward target
            state.y_offset += (state.y_offset_target - state.y_offset) * 0.25;
            needs_render = true;
        } else if state.y_offset != state.y_offset_target {
            state.y_offset = state.y_offset_target;
            needs_render = true;
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
            RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }
        }
    }

    unsafe fn reposition_pill(state: &mut OverlayState) {
        state.dpi_scale = get_dpi_scale(state.hwnd);
        let dpi = state.dpi_scale;

        let mon = active_monitor_rect();
        let mon_w = (mon.right - mon.left) as f32;
        let max_w = sc(BASE_MAX_PILL_WIDTH, dpi).min(mon_w - 2.0 * sc(BASE_SCREEN_H_MARGIN, dpi));

        // Measure text width for dynamic sizing
        let display = if state.interim_text.is_empty() { &state.status_text } else { &state.interim_text };
        let text_w = if !display.is_empty() {
            let is_status_only = state.interim_text.is_empty();
            let font_size = if is_status_only { BASE_STATUS_FONT } else { BASE_INTERIM_FONT };
            measure_text_width(&state.dwrite_factory, display, font_size, is_status_only, dpi)
        } else { 0.0 };

        let icon_space = sc(BASE_TEXT_LEFT, dpi);
        let pad_right = sc(BASE_PAD_RIGHT, dpi);
        let desired_w = icon_space + text_w + pad_right;
        let min_w = sc(BASE_MIN_PILL_WIDTH, dpi);
        let mut w = desired_w.max(min_w).min(max_w);

        // Session-monotonic: only grow during a session
        if state.session_max_w > 0.0 {
            w = w.max(state.session_max_w);
        }
        state.session_max_w = w;

        let h = if state.interim_text.is_empty() {
            sc(BASE_PILL_HEIGHT_SMALL, dpi)
        } else {
            // Measure wrapped text height — full pill is text only (no status row)
            let text_area_w = w - sc(BASE_TEXT_LEFT, dpi) - sc(BASE_PAD_RIGHT, dpi);
            let (total_h, line_h) = measure_text_height(
                &state.dwrite_factory, &state.interim_text, BASE_INTERIM_FONT, text_area_w, dpi,
            );
            let max_text_h = line_h * MAX_VISIBLE_LINES as f32;
            let clamped_h = total_h.min(max_text_h).max(line_h);
            let top_pad = sc(8.0, dpi);
            let bottom_pad = sc(8.0, dpi);
            (top_pad + clamped_h + bottom_pad).max(sc(BASE_PILL_HEIGHT_SMALL, dpi))
        };

        let wi = w.round() as i32;
        let hi = h.round() as i32;
        let x = mon.left + ((mon_w as i32 - wi) / 2);
        let y = mon.bottom - hi - sci(BASE_BOTTOM_MARGIN, dpi);

        state.pill_x = x;
        state.pill_y = y;
        state.pill_w = wi;
        state.pill_h = hi;

        SetWindowPos(state.hwnd, HWND_TOPMOST, x, y, wi, hi, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    unsafe fn show_window(state: &mut OverlayState) {
        if !state.visible {
            ShowWindow(state.hwnd, SW_SHOWNOACTIVATE);
            state.visible = true;
            // Start entrance animation: from below
            state.y_offset = sc(ENTRANCE_LIFT, state.dpi_scale);
        }
        state.y_offset_target = 0.0;
        state.target_alpha = MAX_ALPHA;
    }

    unsafe fn hide_window(state: &mut OverlayState) {
        state.target_alpha = 0;
        // Exit animation: drop down
        state.y_offset_target = sc(EXIT_DROP, state.dpi_scale);
    }
}
