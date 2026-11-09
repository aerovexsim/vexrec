use gdk::prelude::*;
use gtk::prelude::*;
use image::{ImageBuffer, Rgba};
use std::cell::RefCell;
use std::rc::Rc;
use vexrec_core::config::ensure_unique_path;
use vexrec_core::error::VexrecError;
use vexrec_core::geometry::Rect;
use vexrec_pipeline::snapshot::SnapshotEngine;
use vexrec_pipeline::x11_capture::X11CaptureEngine;
use vexrec_portal::PortalClient;
use crate::app_state::{expand_path, AppState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetMode {
    Selection,
    Window,
    WindowRegion,
    Fullscreen,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActionMode {
    Screenshot,
    Record,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragHandle {
    None,
    New,
    Move,
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

#[derive(Clone)]
pub struct OverviewWindow {
    pub xid: u32,
    pub title: String,
    pub class_name: String,
    pub screen_rect: Rect,
    pub slot_rect: (f64, f64, f64, f64),
    pub thumbnail: Option<gdk_pixbuf::Pixbuf>,
    pub sub_pixbuf: Option<gdk_pixbuf::Pixbuf>,
    pub icon: Option<gdk_pixbuf::Pixbuf>,
}

#[derive(Clone)]
pub struct ZoomAnimation {
    pub start_time: std::time::Instant,
    pub duration: std::time::Duration,
    pub from_rect: (f64, f64, f64, f64),
    pub to_rect: (f64, f64, f64, f64),
    pub window_idx: usize,
}

struct OverlayInner {
    image: Option<ImageBuffer<Rgba<u8>, Vec<u8>>>,
    pixbuf: Option<gdk_pixbuf::Pixbuf>,
    crop: Option<Rect>,
    action_mode: ActionMode,
    target_mode: TargetMode,
    mic_enabled: bool,
    desktop_audio_enabled: bool,
    drag_handle: DragHandle,
    drag_start: (f64, f64),
    initial_crop: Option<Rect>,
    mouse_pos: (f64, f64),
    windows: Vec<OverviewWindow>,
    hovered_window: Option<usize>,
    zoom_animation: Option<ZoomAnimation>,
    window_region_selected: Option<usize>,
}

#[derive(Clone)]
pub struct FreezeOverlayWindow {
    window: gtk::Window,
    drawing_area: gtk::DrawingArea,
    inner: Rc<RefCell<OverlayInner>>,
    app_state: AppState,
    btn_done: gtk::Button,
    btn_save_as: gtk::Button,
    btn_record_action: gtk::Button,
    btn_mic: gtk::Button,
    btn_desktop_audio: gtk::Button,
    audio_box: gtk::Box,
    btn_mode_screenshot: gtk::Button,
    btn_mode_record: gtk::Button,
    btn_tgt_selection: gtk::Button,
    btn_tgt_window: gtk::Button,
    btn_tgt_window_region: gtk::Button,
    btn_tgt_fullscreen: gtk::Button,
}

fn rounded_rectangle(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let degrees = std::f64::consts::PI / 180.0;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -90.0 * degrees, 0.0 * degrees);
    cr.arc(x + w - r, y + h - r, r, 0.0 * degrees, 90.0 * degrees);
    cr.arc(x + r, y + h - r, r, 90.0 * degrees, 180.0 * degrees);
    cr.arc(x + r, y + r, r, 180.0 * degrees, 270.0 * degrees);
    cr.close_path();
}

fn layout_overview_windows(windows: &mut [OverviewWindow], screen_w: f64, screen_h: f64) {
    let n = windows.len();
    if n == 0 {
        return;
    }

    let margin_x = 70.0;
    let margin_top = 85.0;
    let margin_bottom = 115.0;
    let avail_w = (screen_w - 2.0 * margin_x).max(100.0);
    let avail_h = (screen_h - margin_top - margin_bottom).max(100.0);

    let (rows, cols) = match n {
        1 => (1, 1),
        2 => (1, 2),
        3 => (1, 3),
        4 => (2, 2),
        5 | 6 => (2, 3),
        7 | 8 => (2, 4),
        9..=12 => (3, 4),
        _ => (3, ((n + 2) / 3).max(4)),
    };

    let gap_x = 28.0;
    let gap_y = 36.0;

    let cell_h = ((avail_h - (rows - 1) as f64 * gap_y) / rows as f64).max(50.0);
    let cell_w = ((avail_w - (cols - 1) as f64 * gap_x) / cols as f64).max(50.0);

    let mut row_counts = vec![n / rows; rows];
    let remainder = n % rows;
    for i in 0..remainder {
        row_counts[i] += 1;
    }

    let mut current_idx = 0;
    for (r, &count) in row_counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let row_total_w = count as f64 * cell_w + (count - 1) as f64 * gap_x;
        let row_start_x = margin_x + (avail_w - row_total_w) / 2.0;
        let row_start_y = margin_top + r as f64 * (cell_h + gap_y);

        for j in 0..count {
            if current_idx >= windows.len() {
                break;
            }
            let cx = row_start_x + j as f64 * (cell_w + gap_x);
            let cy = row_start_y;

            let win = &mut windows[current_idx];
            let native_w = win.screen_rect.width as f64;
            let native_h = win.screen_rect.height as f64;

            let scale = (cell_w / native_w.max(1.0)).min(cell_h / native_h.max(1.0)).min(1.0);
            let thumb_w = (native_w * scale).round().max(20.0);
            let thumb_h = (native_h * scale).round().max(20.0);
            let thumb_x = (cx + (cell_w - thumb_w) / 2.0).round();
            let thumb_y = (cy + (cell_h - thumb_h) / 2.0).round();

            win.slot_rect = (thumb_x, thumb_y, thumb_w, thumb_h);
            current_idx += 1;
        }
    }
}

impl FreezeOverlayWindow {
    pub fn new<FStartRec, FOpenSettings>(
        app_state: AppState,
        on_start_recording: FStartRec,
        on_open_settings: FOpenSettings,
    ) -> Rc<Self>
    where
        FStartRec: Fn(Option<Rect>, bool, bool) + 'static,
        FOpenSettings: Fn() + 'static,
    {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Vexrec Capture");
        window.set_decorated(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_keep_above(true);
        window.set_app_paintable(true);
        window.set_can_focus(true);
        window.set_accept_focus(true);
        window.set_focus_on_map(true);
        window.set_modal(true);
        window.fullscreen();

        if let Some(screen) = gtk::prelude::GtkWindowExt::screen(&window) {
            if let Some(visual) = screen.rgba_visual() {
                window.set_visual(Some(&visual));
            }
        }

        let inner = Rc::new(RefCell::new(OverlayInner {
            image: None,
            pixbuf: None,
            crop: None,
            action_mode: ActionMode::Screenshot,
            target_mode: TargetMode::Selection,
            mic_enabled: true,
            desktop_audio_enabled: true,
            drag_handle: DragHandle::None,
            drag_start: (0.0, 0.0),
            initial_crop: None,
            mouse_pos: (0.0, 0.0),
            windows: Vec::new(),
            hovered_window: None,
            zoom_animation: None,
            window_region_selected: None,
        }));

        let overlay_container = gtk::Overlay::new();

        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_can_focus(true);
        drawing_area.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::POINTER_MOTION_MASK
                | gdk::EventMask::KEY_PRESS_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK,
        );

        overlay_container.add(&drawing_area);

        // Floating Bottom Control Pill
        let bottom_pill = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom_pill.style_context().add_class("freeze-pill");
        bottom_pill.set_halign(gtk::Align::Center);
        bottom_pill.set_valign(gtk::Align::End);
        bottom_pill.set_margin_bottom(32);

        let provider = gtk::CssProvider::new();
        if provider.load_from_data(crate::DARK_CSS.as_bytes()).is_ok() {
            bottom_pill.style_context().add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
            window.style_context().add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
        }

        // 1. Mode Switcher (Screenshot vs Record)
        let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        mode_box.style_context().add_class("pill-group");

        let btn_mode_screenshot = gtk::Button::with_label("Screenshot");
        btn_mode_screenshot.set_can_focus(false);
        btn_mode_screenshot.style_context().add_class("pill-button");
        btn_mode_screenshot.style_context().add_class("active");

        let btn_mode_record = gtk::Button::with_label("Record");
        btn_mode_record.set_can_focus(false);
        btn_mode_record.style_context().add_class("pill-button");

        mode_box.pack_start(&btn_mode_screenshot, false, false, 0);
        mode_box.pack_start(&btn_mode_record, false, false, 0);
        bottom_pill.pack_start(&mode_box, false, false, 0);

        let sep1 = gtk::Separator::new(gtk::Orientation::Vertical);
        sep1.style_context().add_class("pill-separator");
        bottom_pill.pack_start(&sep1, false, false, 0);

        // 2. Target Switcher (Selection, Window, Window Region, Fullscreen)
        let target_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        target_box.style_context().add_class("pill-group");

        let btn_tgt_selection = gtk::Button::with_label("Selection (1)");
        btn_tgt_selection.set_can_focus(false);
        btn_tgt_selection.style_context().add_class("pill-button");
        btn_tgt_selection.style_context().add_class("active");

        let btn_tgt_window = gtk::Button::with_label("Window (2)");
        btn_tgt_window.set_can_focus(false);
        btn_tgt_window.style_context().add_class("pill-button");

        let btn_tgt_window_region = gtk::Button::with_label("Window Region (3)");
        btn_tgt_window_region.set_can_focus(false);
        btn_tgt_window_region.style_context().add_class("pill-button");

        let btn_tgt_fullscreen = gtk::Button::with_label("Full Screen (4)");
        btn_tgt_fullscreen.set_can_focus(false);
        btn_tgt_fullscreen.style_context().add_class("pill-button");

        target_box.pack_start(&btn_tgt_selection, false, false, 0);
        target_box.pack_start(&btn_tgt_window, false, false, 0);
        target_box.pack_start(&btn_tgt_window_region, false, false, 0);
        target_box.pack_start(&btn_tgt_fullscreen, false, false, 0);
        bottom_pill.pack_start(&target_box, false, false, 0);

        // 3. Audio Toggles (shown when Record mode)
        let audio_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        audio_box.style_context().add_class("pill-group");

        let btn_mic = gtk::Button::with_label("Mic: ON");
        btn_mic.set_can_focus(false);
        btn_mic.style_context().add_class("pill-button");
        btn_mic.style_context().add_class("active");

        let btn_desktop_audio = gtk::Button::with_label("Audio: ON");
        btn_desktop_audio.set_can_focus(false);
        btn_desktop_audio.style_context().add_class("pill-button");
        btn_desktop_audio.style_context().add_class("active");

        audio_box.pack_start(&btn_mic, false, false, 0);
        audio_box.pack_start(&btn_desktop_audio, false, false, 0);
        audio_box.set_no_show_all(true);
        audio_box.hide();
        bottom_pill.pack_start(&audio_box, false, false, 0);

        let sep2 = gtk::Separator::new(gtk::Orientation::Vertical);
        sep2.style_context().add_class("pill-separator");
        bottom_pill.pack_start(&sep2, false, false, 0);

        // 4. Action Buttons
        let btn_done = gtk::Button::with_label("Done ↵");
        btn_done.set_can_focus(false);
        btn_done.style_context().add_class("action-btn-done");
        bottom_pill.pack_start(&btn_done, false, false, 0);

        let btn_save_as = gtk::Button::with_label("Save As");
        btn_save_as.set_can_focus(false);
        btn_save_as.style_context().add_class("pill-button");
        bottom_pill.pack_start(&btn_save_as, false, false, 0);

        let btn_record_action = gtk::Button::with_label("Record ↵");
        btn_record_action.set_can_focus(false);
        btn_record_action.style_context().add_class("action-btn-record");
        btn_record_action.set_no_show_all(true);
        btn_record_action.hide();
        bottom_pill.pack_start(&btn_record_action, false, false, 0);

        // 5. Settings button
        let btn_settings = gtk::Button::with_label("Settings");
        btn_settings.set_can_focus(false);
        btn_settings.style_context().add_class("pill-button");
        bottom_pill.pack_start(&btn_settings, false, false, 0);

        // 6. Cancel button
        let btn_cancel = gtk::Button::with_label("Cancel (Esc)");
        btn_cancel.set_can_focus(false);
        btn_cancel.style_context().add_class("pill-button");
        bottom_pill.pack_start(&btn_cancel, false, false, 0);

        overlay_container.add_overlay(&bottom_pill);
        overlay_container.set_overlay_pass_through(&bottom_pill, false);

        window.add(&overlay_container);

        let overlay = Rc::new(Self {
            window,
            drawing_area,
            inner,
            app_state,
            btn_done,
            btn_save_as,
            btn_record_action,
            btn_mic,
            btn_desktop_audio,
            audio_box,
            btn_mode_screenshot,
            btn_mode_record,
            btn_tgt_selection,
            btn_tgt_window,
            btn_tgt_window_region,
            btn_tgt_fullscreen,
        });

        // Set up Draw signal
        let inner_draw = overlay.inner.clone();
        overlay.drawing_area.connect_draw(move |_da, cr| {
            let inner = inner_draw.borrow();
            let win_w = _da.allocated_width() as f64;
            let win_h = _da.allocated_height() as f64;

            let is_overview = (inner.target_mode == TargetMode::Window || inner.target_mode == TargetMode::WindowRegion)
                && inner.window_region_selected.is_none()
                && inner.zoom_animation.is_none();

            if let Some(ref anim) = inner.zoom_animation {
                // Smooth Zoom Animation
                let elapsed = anim.start_time.elapsed().as_secs_f64() / anim.duration.as_secs_f64();
                let t = (1.0 - (1.0 - elapsed.clamp(0.0, 1.0)).powi(3)).clamp(0.0, 1.0);

                if let Some(ref pb) = inner.pixbuf {
                    cr.set_source_pixbuf(pb, 0.0, 0.0);
                    let _ = cr.paint();
                }

                let alpha_mask = 0.78 * (1.0 - t) + 0.45 * t;
                cr.set_source_rgba(0.06, 0.06, 0.09, alpha_mask);
                cr.rectangle(0.0, 0.0, win_w, win_h);
                let _ = cr.fill();

                let other_alpha = (1.0 - t).max(0.0);
                if other_alpha > 0.01 {
                    for (idx, win) in inner.windows.iter().enumerate() {
                        if idx == anim.window_idx {
                            continue;
                        }
                        let (sx, sy, sw, sh) = win.slot_rect;
                        rounded_rectangle(cr, sx, sy, sw, sh, 10.0);
                        cr.save().unwrap();
                        cr.clip();
                        if let Some(ref thumb) = win.thumbnail {
                            cr.set_source_pixbuf(thumb, sx, sy);
                            let _ = cr.paint_with_alpha(other_alpha);
                        }
                        cr.restore().unwrap();

                        rounded_rectangle(cr, sx, sy, sw, sh, 10.0);
                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.18 * other_alpha);
                        cr.set_line_width(1.0);
                        let _ = cr.stroke();
                    }
                }

                if let Some(win) = inner.windows.get(anim.window_idx) {
                    let (fx, fy, fw, fh) = anim.from_rect;
                    let (tx, ty, tw, th) = anim.to_rect;
                    let cur_x = fx + (tx - fx) * t;
                    let cur_y = fy + (ty - fy) * t;
                    let cur_w = fw + (tw - fw) * t;
                    let cur_h = fh + (th - fh) * t;
                    let corner_r = 10.0 * (1.0 - t);

                    rounded_rectangle(cr, cur_x, cur_y, cur_w, cur_h, corner_r);
                    cr.save().unwrap();
                    cr.clip();
                    if let Some(ref pb) = inner.pixbuf {
                        let sx = win.screen_rect.x as f64;
                        let sy = win.screen_rect.y as f64;
                        let sw = win.screen_rect.width as f64;
                        let sh = win.screen_rect.height as f64;
                        cr.translate(cur_x, cur_y);
                        cr.scale(cur_w / sw, cur_h / sh);
                        cr.set_source_pixbuf(pb, -sx, -sy);
                        let _ = cr.paint();
                    }
                    cr.restore().unwrap();

                    rounded_rectangle(cr, cur_x, cur_y, cur_w, cur_h, corner_r);
                    cr.set_source_rgba(0.388, 0.400, 0.945, 0.95);
                    cr.set_line_width(2.5);
                    let _ = cr.stroke();

                    if let Some(ref icon) = win.icon {
                        let icon_alpha = (1.0 - t * 1.5).max(0.0);
                        if icon_alpha > 0.01 {
                            let ix = (cur_x + cur_w / 2.0 - 21.0).round();
                            let iy = (cur_y + cur_h - 21.0).round();
                            cr.set_source_pixbuf(icon, ix, iy);
                            let _ = cr.paint_with_alpha(icon_alpha);
                        }
                    }
                }
            } else if is_overview {
                if let Some(ref pb) = inner.pixbuf {
                    cr.set_source_pixbuf(pb, 0.0, 0.0);
                    let _ = cr.paint();
                }

                // Dark ambient backdrop matching GNOME Activities overview
                cr.set_source_rgba(0.06, 0.06, 0.09, 0.78);
                cr.rectangle(0.0, 0.0, win_w, win_h);
                let _ = cr.fill();

                // Top Header Pill
                let header_text = if inner.target_mode == TargetMode::Window {
                    "Click a window to capture"
                } else {
                    "Click a window to select a region"
                };
                cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                cr.set_font_size(13.0);
                if let Ok(extents) = cr.text_extents(header_text) {
                    let bw = extents.width() + 36.0;
                    let bh = 32.0;
                    let bx = (win_w - bw) / 2.0;
                    let by = 36.0;
                    rounded_rectangle(cr, bx, by, bw, bh, 16.0);
                    cr.set_source_rgba(0.12, 0.12, 0.16, 0.92);
                    let _ = cr.fill();
                    rounded_rectangle(cr, bx, by, bw, bh, 16.0);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.15);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();

                    cr.set_source_rgb(0.9, 0.9, 0.95);
                    cr.move_to(bx + 18.0, by + 21.0);
                    let _ = cr.show_text(header_text);
                }

                // Draw Windows Grid
                for (idx, win) in inner.windows.iter().enumerate() {
                    let is_hovered = inner.hovered_window == Some(idx);
                    let (sx, sy, sw, sh) = win.slot_rect;

                    // Hover Glow / Shadow
                    if is_hovered {
                        cr.set_source_rgba(0.388, 0.400, 0.945, 0.25);
                        rounded_rectangle(cr, sx - 4.0, sy - 4.0, sw + 8.0, sh + 8.0, 14.0);
                        let _ = cr.fill();
                    }

                    // Card Thumbnail
                    rounded_rectangle(cr, sx, sy, sw, sh, 10.0);
                    cr.save().unwrap();
                    cr.clip();
                    if let Some(ref thumb) = win.thumbnail {
                        cr.set_source_pixbuf(thumb, sx, sy);
                        let _ = cr.paint();
                    }
                    cr.restore().unwrap();

                    // Card Border
                    rounded_rectangle(cr, sx, sy, sw, sh, 10.0);
                    if is_hovered {
                        cr.set_source_rgba(0.388, 0.400, 0.945, 0.95);
                        cr.set_line_width(2.5);
                    } else {
                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.20);
                        cr.set_line_width(1.0);
                    }
                    let _ = cr.stroke();

                    // Centered Application Icon Badge floating at bottom edge
                    let icon_size = 42.0;
                    let ix = (sx + sw / 2.0 - icon_size / 2.0).round();
                    let iy = (sy + sh - icon_size / 2.0).round();

                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
                    cr.arc(ix + icon_size / 2.0, iy + icon_size / 2.0 + 2.0, icon_size / 2.0 + 2.0, 0.0, 2.0 * std::f64::consts::PI);
                    let _ = cr.fill();

                    if let Some(ref icon) = win.icon {
                        cr.set_source_pixbuf(icon, ix, iy);
                        let _ = cr.paint();
                    }

                    // Hover Tooltip Badge above thumbnail
                    if is_hovered && !win.title.is_empty() {
                        let mut title_text = win.title.clone();
                        if title_text.len() > 45 {
                            title_text.truncate(42);
                            title_text.push_str("...");
                        }
                        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                        cr.set_font_size(12.0);
                        if let Ok(extents) = cr.text_extents(&title_text) {
                            let tw = extents.width() + 24.0;
                            let th = 26.0;
                            let tx = (sx + sw / 2.0 - tw / 2.0).max(8.0).min(win_w - tw - 8.0);
                            let ty = (sy - th - 8.0).max(8.0);

                            rounded_rectangle(cr, tx, ty, tw, th, 13.0);
                            cr.set_source_rgba(0.09, 0.09, 0.12, 0.95);
                            let _ = cr.fill();
                            rounded_rectangle(cr, tx, ty, tw, th, 13.0);
                            cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
                            cr.set_line_width(1.0);
                            let _ = cr.stroke();

                            cr.set_source_rgb(1.0, 1.0, 1.0);
                            cr.move_to(tx + 12.0, ty + 18.0);
                            let _ = cr.show_text(&title_text);
                        }
                    }
                }
            } else {
                // Free Selection Mode or Zoomed Window Region
                if let Some(ref pb) = inner.pixbuf {
                    cr.set_source_pixbuf(pb, 0.0, 0.0);
                    let _ = cr.paint();
                }

                // Vignette Mask
                if inner.target_mode != TargetMode::Fullscreen {
                    cr.set_fill_rule(cairo::FillRule::EvenOdd);
                    cr.rectangle(0.0, 0.0, win_w, win_h);
                    if let Some(r) = inner.crop {
                        cr.rectangle(r.x as f64, r.y as f64, r.width as f64, r.height as f64);
                    }
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
                    let _ = cr.fill();
                }

                // Header badge in Window Region Mode
                if inner.target_mode == TargetMode::WindowRegion && inner.window_region_selected.is_some() {
                    let win_idx = inner.window_region_selected.unwrap();
                    let win_title = inner.windows.get(win_idx).map(|w| w.title.as_str()).unwrap_or("Window");
                    let mut hint_title = win_title.to_string();
                    if hint_title.len() > 35 {
                        hint_title.truncate(32);
                        hint_title.push_str("...");
                    }
                    let hint_text = format!("Window Region: {} — Esc to Overview", hint_title);
                    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                    cr.set_font_size(12.0);
                    if let Ok(extents) = cr.text_extents(&hint_text) {
                        let bw = extents.width() + 32.0;
                        let bh = 28.0;
                        let bx = (win_w - bw) / 2.0;
                        let by = 36.0;
                        rounded_rectangle(cr, bx, by, bw, bh, 14.0);
                        cr.set_source_rgba(0.09, 0.09, 0.12, 0.92);
                        let _ = cr.fill();
                        rounded_rectangle(cr, bx, by, bw, bh, 14.0);
                        cr.set_source_rgba(0.388, 0.400, 0.945, 0.7);
                        cr.set_line_width(1.0);
                        let _ = cr.stroke();

                        cr.set_source_rgb(0.9, 0.9, 0.95);
                        cr.move_to(bx + 16.0, by + 19.0);
                        let _ = cr.show_text(&hint_text);
                    }
                }

                // Selection Rect & Handles
                if inner.target_mode == TargetMode::Fullscreen {
                    cr.set_source_rgba(0.388, 0.400, 0.945, 0.85);
                    cr.set_line_width(4.0);
                    cr.rectangle(0.0, 0.0, win_w, win_h);
                    let _ = cr.stroke();
                } else if let Some(r) = inner.crop {
                    if r.width > 2 && r.height > 2 {
                        let rx = r.x as f64;
                        let ry = r.y as f64;
                        let rw = r.width as f64;
                        let rh = r.height as f64;

                        // Border
                        cr.set_source_rgba(0.388, 0.400, 0.945, 0.95);
                        cr.set_line_width(2.0);
                        cr.rectangle(rx, ry, rw, rh);
                        let _ = cr.stroke();

                        // 8 Handles
                        let draw_handle = |cr: &cairo::Context, hx: f64, hy: f64| {
                            cr.set_source_rgb(1.0, 1.0, 1.0);
                            cr.rectangle(hx - 4.0, hy - 4.0, 8.0, 8.0);
                            let _ = cr.fill();
                            cr.set_source_rgb(0.388, 0.400, 0.945);
                            cr.set_line_width(1.5);
                            cr.rectangle(hx - 4.0, hy - 4.0, 8.0, 8.0);
                            let _ = cr.stroke();
                        };

                        draw_handle(cr, rx, ry); // NW
                        draw_handle(cr, rx + rw / 2.0, ry); // N
                        draw_handle(cr, rx + rw, ry); // NE
                        draw_handle(cr, rx + rw, ry + rh / 2.0); // E
                        draw_handle(cr, rx + rw, ry + rh); // SE
                        draw_handle(cr, rx + rw / 2.0, ry + rh); // S
                        draw_handle(cr, rx, ry + rh); // SW
                        draw_handle(cr, rx, ry + rh / 2.0); // W

                        // Dimension Tooltip Badge
                        let text = format!("{} × {} px", r.width, r.height);
                        cr.select_font_face("Monospace", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
                        cr.set_font_size(11.0);
                        let extents = cr.text_extents(&text).unwrap();

                        let badge_w = extents.width() + 16.0;
                        let badge_h = 22.0;
                        let badge_x = rx.max(4.0).min(win_w - badge_w - 4.0);
                        let badge_y = (ry - badge_h - 6.0).max(4.0);

                        // Badge background
                        cr.set_source_rgba(0.094, 0.094, 0.106, 0.92);
                        cr.rectangle(badge_x, badge_y, badge_w, badge_h);
                        let _ = cr.fill();
                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.15);
                        cr.set_line_width(1.0);
                        cr.rectangle(badge_x, badge_y, badge_w, badge_h);
                        let _ = cr.stroke();

                        // Badge text
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        cr.move_to(badge_x + 8.0, badge_y + 15.0);
                        let _ = cr.show_text(&text);
                    }
                }
            }

            glib::Propagation::Proceed
        });

        // Set up Mouse Events
        let ov_press = overlay.clone();
        let da_press = overlay.drawing_area.clone();
        overlay.drawing_area.connect_button_press_event(move |_da, ev| {
            if ev.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (ex, ey) = ev.position();
            let mut inner = ov_press.inner.borrow_mut();

            let is_overview = (inner.target_mode == TargetMode::Window || inner.target_mode == TargetMode::WindowRegion)
                && inner.window_region_selected.is_none()
                && inner.zoom_animation.is_none();

            if is_overview {
                let mut clicked_idx = None;
                for (idx, win) in inner.windows.iter().enumerate() {
                    let (sx, sy, sw, sh) = win.slot_rect;
                    if ex >= sx && ex <= sx + sw && ey >= sy && ey <= sy + sh {
                        clicked_idx = Some(idx);
                        break;
                    }
                }

                if let Some(idx) = clicked_idx {
                    if inner.target_mode == TargetMode::Window {
                        let rect = inner.windows[idx].screen_rect;
                        inner.crop = Some(rect);
                        drop(inner);
                        ov_press.confirm_screenshot(true, None);
                        return glib::Propagation::Stop;
                    } else if inner.target_mode == TargetMode::WindowRegion {
                        let slot = inner.windows[idx].slot_rect;
                        let srect = inner.windows[idx].screen_rect;
                        let target_rect = (
                            srect.x as f64,
                            srect.y as f64,
                            srect.width as f64,
                            srect.height as f64,
                        );
                        inner.zoom_animation = Some(ZoomAnimation {
                            start_time: std::time::Instant::now(),
                            duration: std::time::Duration::from_millis(220),
                            from_rect: slot,
                            to_rect: target_rect,
                            window_idx: idx,
                        });
                        drop(inner);

                        let inner_anim = ov_press.inner.clone();
                        let da_anim = da_press.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                            let mut in_ref = inner_anim.borrow_mut();
                            if let Some(ref anim) = in_ref.zoom_animation {
                                let elapsed = anim.start_time.elapsed().as_secs_f64() / anim.duration.as_secs_f64();
                                if elapsed >= 1.0 {
                                    let win_idx = anim.window_idx;
                                    let srect = in_ref.windows.get(win_idx).map(|w| w.screen_rect);
                                    in_ref.zoom_animation = None;
                                    in_ref.window_region_selected = Some(win_idx);
                                    if let Some(sr) = srect {
                                        in_ref.crop = Some(sr);
                                    }
                                    da_anim.queue_draw();
                                    return glib::ControlFlow::Break;
                                }
                            } else {
                                return glib::ControlFlow::Break;
                            }
                            da_anim.queue_draw();
                            glib::ControlFlow::Continue
                        });

                        return glib::Propagation::Stop;
                    }
                }
                return glib::Propagation::Proceed;
            }

            if inner.target_mode == TargetMode::Fullscreen {
                return glib::Propagation::Proceed;
            }

            let handle = if let Some(r) = inner.crop {
                Self::detect_handle(r, ex, ey)
            } else {
                DragHandle::New
            };

            Self::update_cursor(_da, handle);

            inner.drag_handle = handle;
            inner.drag_start = (ex, ey);
            inner.initial_crop = inner.crop;

            if handle == DragHandle::New {
                inner.crop = Some(Rect::new(ex as i32, ey as i32, 0, 0));
                inner.initial_crop = inner.crop;
            }

            da_press.queue_draw();
            glib::Propagation::Proceed
        });

        let ov_motion = overlay.clone();
        let da_motion = overlay.drawing_area.clone();
        overlay.drawing_area.connect_motion_notify_event(move |_da, ev| {
            let (ex, ey) = ev.position();
            let mut inner = ov_motion.inner.borrow_mut();
            inner.mouse_pos = (ex, ey);

            let is_overview = (inner.target_mode == TargetMode::Window || inner.target_mode == TargetMode::WindowRegion)
                && inner.window_region_selected.is_none()
                && inner.zoom_animation.is_none();

            if is_overview {
                let mut hovered = None;
                for (idx, win) in inner.windows.iter().enumerate() {
                    let (sx, sy, sw, sh) = win.slot_rect;
                    if ex >= sx && ex <= sx + sw && ey >= sy && ey <= sy + sh {
                        hovered = Some(idx);
                        break;
                    }
                }

                if inner.hovered_window != hovered {
                    inner.hovered_window = hovered;
                    da_motion.queue_draw();
                }

                if hovered.is_some() {
                    if let Some(gdk_win) = _da.window() {
                        let display = gdk_win.display();
                        let cursor = gdk::Cursor::from_name(&display, "pointer")
                            .or_else(|| gdk::Cursor::for_display(&display, gdk::CursorType::Hand2));
                        gdk_win.set_cursor(cursor.as_ref());
                    }
                } else {
                    if let Some(gdk_win) = _da.window() {
                        gdk_win.set_cursor(None);
                    }
                }
                return glib::Propagation::Proceed;
            }

            let active_handle = if inner.drag_handle != DragHandle::None {
                inner.drag_handle
            } else if let Some(r) = inner.crop {
                Self::detect_handle(r, ex, ey)
            } else {
                DragHandle::New
            };
            Self::update_cursor(_da, active_handle);

            if inner.drag_handle != DragHandle::None {
                let (sx, sy) = inner.drag_start;
                let dx = ex - sx;
                let dy = ey - sy;

                let bounds = if let Some(widx) = inner.window_region_selected {
                    inner.windows.get(widx).map(|w| w.screen_rect)
                } else {
                    None
                };

                if inner.drag_handle == DragHandle::New {
                    let mut x1 = sx.min(ex);
                    let mut y1 = sy.min(ey);
                    let mut x2 = sx.max(ex);
                    let mut y2 = sy.max(ey);

                    if let Some(b) = bounds {
                        x1 = x1.clamp(b.x as f64, (b.x + b.width as i32) as f64);
                        y1 = y1.clamp(b.y as f64, (b.y + b.height as i32) as f64);
                        x2 = x2.clamp(b.x as f64, (b.x + b.width as i32) as f64);
                        y2 = y2.clamp(b.y as f64, (b.y + b.height as i32) as f64);
                    }

                    inner.crop = Some(Rect::new(
                        x1 as i32,
                        y1 as i32,
                        (x2 - x1).max(0.0) as u32,
                        (y2 - y1).max(0.0) as u32,
                    ));
                } else if inner.drag_handle == DragHandle::Move {
                    if let Some(init) = inner.initial_crop {
                        let (min_x, max_x, min_y, max_y) = if let Some(b) = bounds {
                            (b.x, b.x + b.width as i32 - init.width as i32, b.y, b.y + b.height as i32 - init.height as i32)
                        } else {
                            let win_w = _da.allocated_width();
                            let win_h = _da.allocated_height();
                            (0, (win_w - init.width as i32).max(0), 0, (win_h - init.height as i32).max(0))
                        };
                        let nx = (init.x + dx as i32).clamp(min_x, max_x.max(min_x));
                        let ny = (init.y + dy as i32).clamp(min_y, max_y.max(min_y));
                        inner.crop = Some(Rect::new(nx, ny, init.width, init.height));
                    }
                } else if let Some(init) = inner.initial_crop {
                    let mut x = init.x;
                    let mut y = init.y;
                    let mut w = init.width as i32;
                    let mut h = init.height as i32;

                    match inner.drag_handle {
                        DragHandle::Nw => {
                            x = (init.x + dx as i32).min(init.x + w - 10);
                            y = (init.y + dy as i32).min(init.y + h - 10);
                            w = (init.x + w) - x;
                            h = (init.y + h) - y;
                        }
                        DragHandle::N => {
                            y = (init.y + dy as i32).min(init.y + h - 10);
                            h = (init.y + h) - y;
                        }
                        DragHandle::Ne => {
                            y = (init.y + dy as i32).min(init.y + h - 10);
                            w = (init.width as i32 + dx as i32).max(10);
                            h = (init.y + h) - y;
                        }
                        DragHandle::E => {
                            w = (init.width as i32 + dx as i32).max(10);
                        }
                        DragHandle::Se => {
                            w = (init.width as i32 + dx as i32).max(10);
                            h = (init.height as i32 + dy as i32).max(10);
                        }
                        DragHandle::S => {
                            h = (init.height as i32 + dy as i32).max(10);
                        }
                        DragHandle::Sw => {
                            x = (init.x + dx as i32).min(init.x + w - 10);
                            w = (init.x + w) - x;
                            h = (init.height as i32 + dy as i32).max(10);
                        }
                        DragHandle::W => {
                            x = (init.x + dx as i32).min(init.x + w - 10);
                            w = (init.x + w) - x;
                        }
                        _ => {}
                    }

                    if let Some(b) = bounds {
                        x = x.clamp(b.x, b.x + b.width as i32 - 10);
                        y = y.clamp(b.y, b.y + b.height as i32 - 10);
                        w = w.min(b.x + b.width as i32 - x);
                        h = h.min(b.y + b.height as i32 - y);
                    }

                    inner.crop = Some(Rect::new(x.max(0), y.max(0), w.max(1) as u32, h.max(1) as u32));
                }

                da_motion.queue_draw();
            }

            glib::Propagation::Proceed
        });

        let ov_release = overlay.clone();
        let da_release = overlay.drawing_area.clone();
        overlay.drawing_area.connect_button_release_event(move |_da, ev| {
            let (ex, ey) = ev.position();
            let mut inner = ov_release.inner.borrow_mut();
            inner.drag_handle = DragHandle::None;
            inner.initial_crop = None;

            let is_overview = (inner.target_mode == TargetMode::Window || inner.target_mode == TargetMode::WindowRegion)
                && inner.window_region_selected.is_none()
                && inner.zoom_animation.is_none();

            if is_overview {
                return glib::Propagation::Proceed;
            }

            let handle = if let Some(r) = inner.crop {
                Self::detect_handle(r, ex, ey)
            } else {
                DragHandle::New
            };
            Self::update_cursor(_da, handle);

            da_release.queue_draw();
            glib::Propagation::Proceed
        });

        overlay.drawing_area.connect_leave_notify_event(move |_da, _ev| {
            if let Some(gdk_win) = _da.window() {
                gdk_win.set_cursor(None);
            }
            glib::Propagation::Proceed
        });

        // Keyboard Shortcuts
        let start_rec_shared = Rc::new(on_start_recording);
        let handle_key = {
            let overlay_key = overlay.clone();
            let start_rec_key = start_rec_shared.clone();
            Rc::new(move |ev: &gdk::EventKey| -> glib::Propagation {
                match ev.keyval() {
                    gdk::keys::constants::Escape => {
                        let is_zoomed = overlay_key.inner.borrow().window_region_selected.is_some();
                        if is_zoomed {
                            let mut in_ref = overlay_key.inner.borrow_mut();
                            in_ref.window_region_selected = None;
                            in_ref.crop = None;
                            drop(in_ref);
                            overlay_key.drawing_area.queue_draw();
                        } else {
                            overlay_key.cancel();
                        }
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::Print => {
                        overlay_key.show_freeze();
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::Return | gdk::keys::constants::KP_Enter => {
                        let mode = overlay_key.inner.borrow().action_mode;
                        if mode == ActionMode::Screenshot {
                            overlay_key.confirm_screenshot(true, None);
                        } else {
                            let crop = overlay_key.get_actual_crop();
                            let mic = overlay_key.inner.borrow().mic_enabled;
                            let audio = overlay_key.inner.borrow().desktop_audio_enabled;
                            overlay_key.cancel();
                            start_rec_key(crop, mic, audio);
                        }
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::c | gdk::keys::constants::C
                        if ev.state().contains(gdk::ModifierType::CONTROL_MASK) =>
                    {
                        overlay_key.confirm_screenshot(true, None);
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::s | gdk::keys::constants::S
                        if ev.state().contains(gdk::ModifierType::CONTROL_MASK) =>
                    {
                        overlay_key.save_screenshot_as();
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::_1 => {
                        overlay_key.set_target_mode(TargetMode::Selection);
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::_2 => {
                        overlay_key.set_target_mode(TargetMode::Window);
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::_3 => {
                        overlay_key.set_target_mode(TargetMode::WindowRegion);
                        glib::Propagation::Stop
                    }
                    gdk::keys::constants::_4 => {
                        overlay_key.set_target_mode(TargetMode::Fullscreen);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            })
        };

        let hk1 = handle_key.clone();
        overlay.window.connect_key_press_event(move |_win, ev| hk1(ev));
        let hk2 = handle_key;
        overlay.drawing_area.connect_key_press_event(move |_da, ev| hk2(ev));

        // Window Map / Unmap Hooks for Keyboard Grab
        let ov_map = overlay.clone();
        overlay.window.connect_map(move |_| {
            ov_map.grab_keyboard();
        });

        let ov_unmap = overlay.clone();
        overlay.window.connect_unmap(move |_| {
            ov_unmap.ungrab_keyboard();
        });

        // Button Click Handlers
        let ov_done = overlay.clone();
        overlay.btn_done.connect_clicked(move |_| {
            ov_done.confirm_screenshot(true, None);
        });

        let ov_save_as = overlay.clone();
        overlay.btn_save_as.connect_clicked(move |_| {
            ov_save_as.save_screenshot_as();
        });

        let ov_rec = overlay.clone();
        let start_rec_btn = start_rec_shared.clone();
        overlay.btn_record_action.connect_clicked(move |_| {
            let crop = ov_rec.get_actual_crop();
            let mic = ov_rec.inner.borrow().mic_enabled;
            let audio = ov_rec.inner.borrow().desktop_audio_enabled;
            ov_rec.cancel();
            start_rec_btn(crop, mic, audio);
        });

        let ov_mode_shot = overlay.clone();
        overlay.btn_mode_screenshot.connect_clicked(move |_| {
            ov_mode_shot.set_action_mode(ActionMode::Screenshot);
        });

        let ov_mode_rec = overlay.clone();
        overlay.btn_mode_record.connect_clicked(move |_| {
            ov_mode_rec.set_action_mode(ActionMode::Record);
        });

        let ov_tgt_sel = overlay.clone();
        overlay.btn_tgt_selection.connect_clicked(move |_| {
            ov_tgt_sel.set_target_mode(TargetMode::Selection);
        });

        let ov_tgt_win = overlay.clone();
        overlay.btn_tgt_window.connect_clicked(move |_| {
            ov_tgt_win.set_target_mode(TargetMode::Window);
        });

        let ov_tgt_winreg = overlay.clone();
        overlay.btn_tgt_window_region.connect_clicked(move |_| {
            ov_tgt_winreg.set_target_mode(TargetMode::WindowRegion);
        });

        let ov_tgt_full = overlay.clone();
        overlay.btn_tgt_fullscreen.connect_clicked(move |_| {
            ov_tgt_full.set_target_mode(TargetMode::Fullscreen);
        });

        let ov_mic = overlay.clone();
        overlay.btn_mic.connect_clicked(move |_| {
            let mut inner = ov_mic.inner.borrow_mut();
            inner.mic_enabled = !inner.mic_enabled;
            ov_mic.btn_mic.set_label(if inner.mic_enabled { "Mic: ON" } else { "Mic: OFF" });
            if inner.mic_enabled {
                ov_mic.btn_mic.style_context().add_class("active");
            } else {
                ov_mic.btn_mic.style_context().remove_class("active");
            }
        });

        let ov_audio = overlay.clone();
        overlay.btn_desktop_audio.connect_clicked(move |_| {
            let mut inner = ov_audio.inner.borrow_mut();
            inner.desktop_audio_enabled = !inner.desktop_audio_enabled;
            ov_audio.btn_desktop_audio.set_label(if inner.desktop_audio_enabled { "Audio: ON" } else { "Audio: OFF" });
            if inner.desktop_audio_enabled {
                ov_audio.btn_desktop_audio.style_context().add_class("active");
            } else {
                ov_audio.btn_desktop_audio.style_context().remove_class("active");
            }
        });

        let ov_cancel = overlay.clone();
        btn_cancel.connect_clicked(move |_| {
            ov_cancel.cancel();
        });

        let ov_settings = overlay.clone();
        btn_settings.connect_clicked(move |_| {
            ov_settings.cancel();
            on_open_settings();
        });

        overlay
    }

    fn detect_handle(r: Rect, x: f64, y: f64) -> DragHandle {
        let rx = r.x as f64;
        let ry = r.y as f64;
        let rw = r.width as f64;
        let rh = r.height as f64;
        let r_bound = (rw.min(rh) / 2.0).clamp(6.0, 14.0);

        let dist = |hx: f64, hy: f64| -> f64 {
            let dx = x - hx;
            let dy = y - hy;
            (dx * dx + dy * dy).sqrt()
        };

        // Prioritize corners so picking corner handles is immediate and responsive
        if dist(rx, ry) <= r_bound {
            DragHandle::Nw
        } else if dist(rx + rw, ry) <= r_bound {
            DragHandle::Ne
        } else if dist(rx + rw, ry + rh) <= r_bound {
            DragHandle::Se
        } else if dist(rx, ry + rh) <= r_bound {
            DragHandle::Sw
        } else if dist(rx + rw / 2.0, ry) <= r_bound {
            DragHandle::N
        } else if dist(rx + rw, ry + rh / 2.0) <= r_bound {
            DragHandle::E
        } else if dist(rx + rw / 2.0, ry + rh) <= r_bound {
            DragHandle::S
        } else if dist(rx, ry + rh / 2.0) <= r_bound {
            DragHandle::W
        } else if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
            DragHandle::Move
        } else {
            DragHandle::New
        }
    }

    fn update_cursor(da: &gtk::DrawingArea, handle: DragHandle) {
        if let Some(gdk_win) = da.window() {
            let cursor_name = match handle {
                DragHandle::Nw
                | DragHandle::Ne
                | DragHandle::Se
                | DragHandle::Sw
                | DragHandle::N
                | DragHandle::S
                | DragHandle::E
                | DragHandle::W => "pointer",
                DragHandle::Move => "move",
                DragHandle::New => "crosshair",
                DragHandle::None => "default",
            };
            let display = gdk_win.display();
            let cursor = gdk::Cursor::from_name(&display, cursor_name).or_else(|| match cursor_name {
                "pointer" => gdk::Cursor::for_display(&display, gdk::CursorType::Hand2),
                "move" => gdk::Cursor::for_display(&display, gdk::CursorType::Fleur),
                "crosshair" => gdk::Cursor::for_display(&display, gdk::CursorType::Crosshair),
                _ => None,
            });
            gdk_win.set_cursor(cursor.as_ref());
        }
    }

    pub fn set_action_mode(&self, mode: ActionMode) {
        self.inner.borrow_mut().action_mode = mode;
        if mode == ActionMode::Screenshot {
            self.btn_mode_screenshot.style_context().add_class("active");
            self.btn_mode_record.style_context().remove_class("active");
            self.btn_done.show();
            self.btn_save_as.show();
            self.btn_record_action.hide();
            self.audio_box.hide();
        } else {
            self.btn_mode_screenshot.style_context().remove_class("active");
            self.btn_mode_record.style_context().add_class("active");
            self.btn_done.hide();
            self.btn_save_as.hide();
            self.btn_record_action.show();
            self.audio_box.show();
        }
        self.drawing_area.queue_draw();
    }

    pub fn set_target_mode(&self, target: TargetMode) {
        let mut inner = self.inner.borrow_mut();
        inner.target_mode = target;
        inner.zoom_animation = None;
        inner.window_region_selected = None;
        self.btn_tgt_selection.style_context().remove_class("active");
        self.btn_tgt_window.style_context().remove_class("active");
        self.btn_tgt_window_region.style_context().remove_class("active");
        self.btn_tgt_fullscreen.style_context().remove_class("active");

        match target {
            TargetMode::Selection => {
                self.btn_tgt_selection.style_context().add_class("active");
            }
            TargetMode::Window => {
                self.btn_tgt_window.style_context().add_class("active");
            }
            TargetMode::WindowRegion => {
                self.btn_tgt_window_region.style_context().add_class("active");
            }
            TargetMode::Fullscreen => {
                self.btn_tgt_fullscreen.style_context().add_class("active");
            }
        }
        drop(inner);
        self.drawing_area.queue_draw();
    }

    pub fn get_actual_crop(&self) -> Option<Rect> {
        let inner = self.inner.borrow();
        if inner.target_mode == TargetMode::Fullscreen {
            return None;
        }
        inner.crop
    }

    pub fn cancel(&self) {
        self.ungrab_keyboard();
        self.window.hide();
        let mut inner = self.inner.borrow_mut();
        inner.crop = None;
        inner.image = None;
        inner.pixbuf = None;
        inner.windows.clear();
        inner.hovered_window = None;
        inner.zoom_animation = None;
        inner.window_region_selected = None;
        inner.action_mode = ActionMode::Screenshot;
    }

    pub fn confirm_screenshot(&self, copy_to_clipboard: bool, custom_path: Option<std::path::PathBuf>) {
        let (img, crop) = {
            let inner = self.inner.borrow();
            (inner.image.clone(), inner.crop)
        };

        self.cancel();

        if let Some(mut image) = img {
            if let Some(r) = crop {
                let (w, h) = image.dimensions();
                let rx = (r.x as u32).min(w.saturating_sub(1));
                let ry = (r.y as u32).min(h.saturating_sub(1));
                let rw = r.width.min(w.saturating_sub(rx));
                let rh = r.height.min(h.saturating_sub(ry));
                if rw > 0 && rh > 0 {
                    image = image::imageops::crop_imm(&image, rx, ry, rw, rh).to_image();
                }
            }

            if copy_to_clipboard {
                let (w, h) = image.dimensions();
                let raw = image.clone().into_raw();
                let bytes = glib::Bytes::from(&raw);
                let pb = gdk_pixbuf::Pixbuf::from_bytes(
                    &bytes,
                    gdk_pixbuf::Colorspace::Rgb,
                    true,
                    8,
                    w as i32,
                    h as i32,
                    (w * 4) as i32,
                );
                let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
                clipboard.set_image(&pb);
                clipboard.store();

                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_image(arboard::ImageData {
                        width: w as usize,
                        height: h as usize,
                        bytes: std::borrow::Cow::Borrowed(&raw),
                    });
                }
            }

            let cfg = self.app_state.get_config();
            let out_path = custom_path.unwrap_or_else(|| {
                let dir = expand_path(&cfg.storage.screenshot_dir);
                let _ = std::fs::create_dir_all(&dir);
                let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
                ensure_unique_path(dir.join(format!("Vexrec_{}.png", now)))
            });

            if let Some(parent) = out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            if let Ok(()) = SnapshotEngine::save_image(&image, &out_path) {
                crate::commands::notify_with_open_action(
                    "camera-photo",
                    "Screenshot Captured",
                    &format!("Saved to {}", out_path.display()),
                    &out_path,
                );
            }
        }
    }

    pub fn save_screenshot_as(&self) {
        self.ungrab_keyboard();
        let chooser = gtk::FileChooserNative::new(
            Some("Save Screenshot As"),
            Some(&self.window),
            gtk::FileChooserAction::Save,
            Some("Save"),
            Some("Cancel"),
        );
        chooser.set_do_overwrite_confirmation(true);
        let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        chooser.set_current_name(&format!("Vexrec_{}.png", now));

        if chooser.run() == gtk::ResponseType::Accept {
            if let Some(chosen) = chooser.filename() {
                self.confirm_screenshot(true, Some(chosen));
            }
        } else if self.window.is_visible() {
            self.grab_keyboard();
        }
    }

    pub fn try_grab_seat(&self) -> bool {
        if let Some(display) = gdk::Display::default() {
            if let Some(seat) = display.default_seat() {
                if let Some(gdk_win) = self.window.window() {
                    gdk_win.focus(0);
                    let status = seat.grab(
                        &gdk_win,
                        gdk::SeatCapabilities::KEYBOARD,
                        false,
                        None,
                        None,
                        None,
                    );
                    return status == gdk::GrabStatus::Success;
                }
            }
        }
        false
    }

    pub fn grab_keyboard(&self) {
        self.window.grab_focus();
        self.drawing_area.grab_focus();
        self.window.grab_add();
        let _ = self.try_grab_seat();

        // Retry seat grab in case window manager hotkey was still releasing
        let retry = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(30), move || {
            if retry.window.is_visible() {
                if !retry.try_grab_seat() {
                    return glib::ControlFlow::Continue;
                }
            }
            glib::ControlFlow::Break
        });
    }

    pub fn ungrab_keyboard(&self) {
        self.window.grab_remove();
        if let Some(display) = gdk::Display::default() {
            if let Some(seat) = display.default_seat() {
                seat.ungrab();
            }
        }
    }

    pub fn show_freeze(&self) {
        // If already visible, unmap and wait for compositor repaint to avoid self-capture
        if self.window.is_visible() {
            self.ungrab_keyboard();
            self.window.hide();
            while gtk::events_pending() {
                gtk::main_iteration();
            }
            if let Some(display) = gdk::Display::default() {
                display.sync();
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
        }

        // Fast capture: Direct X11 or Portal
        let img_res: Result<ImageBuffer<Rgba<u8>, Vec<u8>>, VexrecError> = if X11CaptureEngine::is_available() {
            X11CaptureEngine::capture_screen(None)
        } else {
            let rt = tokio::runtime::Handle::try_current();
            if let Ok(handle) = rt {
                tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        let portal = PortalClient::new();
                        let uri = portal.request_screenshot(false).await.map_err(|e| VexrecError::Portal(e.to_string()))?;
                        let p = uri.to_file_path().map_err(|_| VexrecError::Pipeline("Bad URI".to_string()))?;
                        let loaded = image::open(&p).map_err(|e| VexrecError::Pipeline(e.to_string()))?.to_rgba8();
                        let _ = std::fs::remove_file(&p);
                        Ok(loaded)
                    })
                })
            } else {
                Err(VexrecError::Pipeline("Capture unavailable".into()))
            }
        };

        if let Ok(img) = img_res {
            let (w, h) = img.dimensions();
            let raw_bytes = img.clone().into_raw();
            let bytes = glib::Bytes::from(&raw_bytes);
            let pixbuf = gdk_pixbuf::Pixbuf::from_bytes(
                &bytes,
                gdk_pixbuf::Colorspace::Rgb,
                true,
                8,
                w as i32,
                h as i32,
                (w * 4) as i32,
            );

            // Enumerate open client windows for Overview mode
            let win_infos = X11CaptureEngine::enumerate_client_windows();
            let mut overview_windows = Vec::new();

            for info in win_infos {
                let px = info.rect.x.clamp(0, (w - 1) as i32);
                let py = info.rect.y.clamp(0, (h - 1) as i32);
                let pw = (info.rect.width as i32).min((w as i32) - px).max(1);
                let ph = (info.rect.height as i32).min((h as i32) - py).max(1);
                let thumb_pb = pixbuf.new_subpixbuf(px, py, pw, ph);

                let icon_pb = if let Some((iw, ih, ref rgba)) = info.icon_rgba {
                    let icon_bytes = glib::Bytes::from(rgba);
                    let pb = gdk_pixbuf::Pixbuf::from_bytes(
                        &icon_bytes,
                        gdk_pixbuf::Colorspace::Rgb,
                        true,
                        8,
                        iw as i32,
                        ih as i32,
                        (iw * 4) as i32,
                    );
                    pb.scale_simple(42, 42, gdk_pixbuf::InterpType::Bilinear)
                } else {
                    gtk::IconTheme::default().and_then(|t| {
                        t.load_icon(&info.class_name.to_lowercase(), 42, gtk::IconLookupFlags::GENERIC_FALLBACK).ok().flatten()
                    })
                };

                overview_windows.push(OverviewWindow {
                    xid: info.xid,
                    title: info.title,
                    class_name: info.class_name,
                    screen_rect: info.rect,
                    slot_rect: (0.0, 0.0, 0.0, 0.0),
                    thumbnail: None,
                    sub_pixbuf: Some(thumb_pb),
                    icon: icon_pb,
                });
            }

            layout_overview_windows(&mut overview_windows, w as f64, h as f64);

            for ow in &mut overview_windows {
                let (_, _, sw, sh) = ow.slot_rect;
                if let Some(ref sub) = ow.sub_pixbuf {
                    ow.thumbnail = sub.scale_simple(sw as i32, sh as i32, gdk_pixbuf::InterpType::Bilinear);
                }
            }

            let cfg = self.app_state.get_config();
            let mut inner = self.inner.borrow_mut();
            inner.image = Some(img);
            inner.pixbuf = Some(pixbuf);
            inner.windows = overview_windows;
            inner.hovered_window = None;
            inner.zoom_animation = None;
            inner.window_region_selected = None;

            // Restore last region or default centered box
            if let Some(last_r) = cfg.screenshot.last_region {
                inner.crop = Some(last_r);
            } else {
                let default_w = 800.min(w.saturating_sub(100));
                let default_h = 500.min(h.saturating_sub(100));
                let default_x = (w.saturating_sub(default_w)) / 2;
                let default_y = (h.saturating_sub(default_h)) / 2;
                inner.crop = Some(Rect::new(default_x as i32, default_y as i32, default_w, default_h));
            }
            drop(inner);

            // By default it should always be in Screenshot mode, Record mode should never be the default
            self.set_action_mode(ActionMode::Screenshot);

            self.window.show_all();
            self.window.present();
            self.drawing_area.queue_draw();

            while gtk::events_pending() {
                gtk::main_iteration();
            }
            self.grab_keyboard();
        }
    }
}
