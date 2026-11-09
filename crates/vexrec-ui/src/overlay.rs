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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TargetMode {
    Selection,
    Window,
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
    btn_tgt_fullscreen: gtk::Button,
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
        }));

        let overlay_container = gtk::Overlay::new();

        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_can_focus(true);
        drawing_area.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::POINTER_MOTION_MASK
                | gdk::EventMask::KEY_PRESS_MASK,
        );

        overlay_container.add(&drawing_area);

        // Floating Bottom Control Pill
        let bottom_pill = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom_pill.style_context().add_class("freeze-pill");
        bottom_pill.set_halign(gtk::Align::Center);
        bottom_pill.set_valign(gtk::Align::End);
        bottom_pill.set_margin_bottom(32);

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

        // 2. Target Switcher (Selection, Window, Fullscreen)
        let target_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        target_box.style_context().add_class("pill-group");

        let btn_tgt_selection = gtk::Button::with_label("Selection (1)");
        btn_tgt_selection.set_can_focus(false);
        btn_tgt_selection.style_context().add_class("pill-button");
        btn_tgt_selection.style_context().add_class("active");

        let btn_tgt_window = gtk::Button::with_label("Window (2)");
        btn_tgt_window.set_can_focus(false);
        btn_tgt_window.style_context().add_class("pill-button");

        let btn_tgt_fullscreen = gtk::Button::with_label("Full Screen (3)");
        btn_tgt_fullscreen.set_can_focus(false);
        btn_tgt_fullscreen.style_context().add_class("pill-button");

        target_box.pack_start(&btn_tgt_selection, false, false, 0);
        target_box.pack_start(&btn_tgt_window, false, false, 0);
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
            btn_tgt_fullscreen,
        });

        // Set up Draw signal
        let inner_draw = overlay.inner.clone();
        overlay.drawing_area.connect_draw(move |_da, cr| {
            let inner = inner_draw.borrow();
            let win_w = _da.allocated_width() as f64;
            let win_h = _da.allocated_height() as f64;

            // 1. Paint Screenshot Pixbuf
            if let Some(ref pb) = inner.pixbuf {
                cr.set_source_pixbuf(pb, 0.0, 0.0);
                let _ = cr.paint();
            }

            // 2. Dimming Vignette Mask
            if inner.target_mode != TargetMode::Fullscreen {
                cr.set_fill_rule(cairo::FillRule::EvenOdd);
                cr.rectangle(0.0, 0.0, win_w, win_h);
                if let Some(r) = inner.crop {
                    cr.rectangle(r.x as f64, r.y as f64, r.width as f64, r.height as f64);
                }
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
                let _ = cr.fill();
            }

            // 3. Selection Rect & Handles
            if inner.target_mode == TargetMode::Fullscreen {
                cr.set_source_rgba(0.388, 0.400, 0.945, 0.85); // Indigo
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

            glib::Propagation::Proceed
        });

        // Set up Mouse Events
        let inner_press = overlay.inner.clone();
        let da_press = overlay.drawing_area.clone();
        overlay.drawing_area.connect_button_press_event(move |_da, ev| {
            if ev.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let (ex, ey) = ev.position();
            let mut inner = inner_press.borrow_mut();

            if inner.target_mode == TargetMode::Fullscreen {
                return glib::Propagation::Proceed;
            }

            let handle = if let Some(r) = inner.crop {
                Self::detect_handle(r, ex, ey)
            } else {
                DragHandle::New
            };

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

        let inner_motion = overlay.inner.clone();
        let da_motion = overlay.drawing_area.clone();
        overlay.drawing_area.connect_motion_notify_event(move |_da, ev| {
            let (ex, ey) = ev.position();
            let mut inner = inner_motion.borrow_mut();
            inner.mouse_pos = (ex, ey);

            if inner.drag_handle != DragHandle::None {
                let (sx, sy) = inner.drag_start;
                let dx = ex - sx;
                let dy = ey - sy;

                if inner.drag_handle == DragHandle::New {
                    let x1 = sx.min(ex);
                    let y1 = sy.min(ey);
                    let x2 = sx.max(ex);
                    let y2 = sy.max(ey);
                    inner.crop = Some(Rect::new(
                        x1 as i32,
                        y1 as i32,
                        (x2 - x1).max(0.0) as u32,
                        (y2 - y1).max(0.0) as u32,
                    ));
                } else if inner.drag_handle == DragHandle::Move {
                    if let Some(init) = inner.initial_crop {
                        let win_w = _da.allocated_width();
                        let win_h = _da.allocated_height();
                        let nx = (init.x + dx as i32).clamp(0, (win_w - init.width as i32).max(0));
                        let ny = (init.y + dy as i32).clamp(0, (win_h - init.height as i32).max(0));
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

                    inner.crop = Some(Rect::new(x.max(0), y.max(0), w.max(1) as u32, h.max(1) as u32));
                }

                da_motion.queue_draw();
            }

            glib::Propagation::Proceed
        });

        let inner_release = overlay.inner.clone();
        let da_release = overlay.drawing_area.clone();
        overlay.drawing_area.connect_button_release_event(move |_da, _ev| {
            let mut inner = inner_release.borrow_mut();
            inner.drag_handle = DragHandle::None;
            inner.initial_crop = None;
            da_release.queue_draw();
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
                        overlay_key.cancel();
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
        const R: f64 = 10.0;

        let dist = |hx: f64, hy: f64| -> f64 {
            let dx = x - hx;
            let dy = y - hy;
            (dx * dx + dy * dy).sqrt()
        };

        if dist(rx, ry) <= R {
            DragHandle::Nw
        } else if dist(rx + rw / 2.0, ry) <= R {
            DragHandle::N
        } else if dist(rx + rw, ry) <= R {
            DragHandle::Ne
        } else if dist(rx + rw, ry + rh / 2.0) <= R {
            DragHandle::E
        } else if dist(rx + rw, ry + rh) <= R {
            DragHandle::Se
        } else if dist(rx + rw / 2.0, ry + rh) <= R {
            DragHandle::S
        } else if dist(rx, ry + rh) <= R {
            DragHandle::Sw
        } else if dist(rx, ry + rh / 2.0) <= R {
            DragHandle::W
        } else if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
            DragHandle::Move
        } else {
            DragHandle::New
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
        self.inner.borrow_mut().target_mode = target;
        self.btn_tgt_selection.style_context().remove_class("active");
        self.btn_tgt_window.style_context().remove_class("active");
        self.btn_tgt_fullscreen.style_context().remove_class("active");

        match target {
            TargetMode::Selection => {
                self.btn_tgt_selection.style_context().add_class("active");
            }
            TargetMode::Window => {
                self.btn_tgt_window.style_context().add_class("active");
            }
            TargetMode::Fullscreen => {
                self.btn_tgt_fullscreen.style_context().add_class("active");
            }
        }
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

            let cfg = self.app_state.get_config();
            let mut inner = self.inner.borrow_mut();
            inner.image = Some(img);
            inner.pixbuf = Some(pixbuf);

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
