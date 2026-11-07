use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

pub struct RecordingIndicatorWindow {
    window: gtk::Window,
    time_label: gtk::Label,
    timer_source: Rc<RefCell<Option<glib::SourceId>>>,
    start_time: Rc<RefCell<Option<Instant>>>,
}

impl RecordingIndicatorWindow {
    pub fn new<F>(on_stop: F) -> Self
    where
        F: Fn() + 'static,
    {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Vexrec Indicator");
        window.set_decorated(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_keep_above(true);
        window.set_app_paintable(true);
        window.set_default_size(100, 28);
        window.set_resizable(false);

        if let Some(screen) = gtk::prelude::GtkWindowExt::screen(&window) {
            if let Some(visual) = screen.rgba_visual() {
                window.set_visual(Some(&visual));
            }
        }

        // Draw transparent background for window
        window.connect_draw(|_win, cr| {
            cr.set_operator(cairo::Operator::Source);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            let _ = cr.paint();
            glib::Propagation::Proceed
        });

        let button = gtk::Button::new();
        button.set_tooltip_text(Some("Stop Recording (Click)"));
        button.style_context().add_class("indicator-pill");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.set_halign(gtk::Align::Center);
        hbox.set_valign(gtk::Align::Center);

        // Red pulsing dot
        let dot = gtk::DrawingArea::new();
        dot.set_size_request(10, 10);
        dot.connect_draw(|_widget, cr| {
            cr.set_operator(cairo::Operator::Over);
            // Red circle
            cr.set_source_rgb(0.937, 0.267, 0.267);
            cr.arc(5.0, 5.0, 4.5, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.fill();
            glib::Propagation::Proceed
        });

        let time_label = gtk::Label::new(Some("00:00"));
        time_label.style_context().add_class("indicator-time");

        hbox.pack_start(&dot, false, false, 0);
        hbox.pack_start(&time_label, false, false, 0);
        button.add(&hbox);
        window.add(&button);

        button.connect_clicked(move |_| {
            on_stop();
        });

        let timer_source = Rc::new(RefCell::new(None));
        let start_time = Rc::new(RefCell::new(None));

        Self {
            window,
            time_label,
            timer_source,
            start_time,
        }
    }

    pub fn show_indicator(&self) {
        let screen_w = gdk::Display::default()
            .and_then(|d| d.primary_monitor().or_else(|| d.monitor(0)))
            .map(|m| m.geometry().width())
            .unwrap_or(1920);
        let x = (screen_w - 120).max(0);
        let y = 38;

        self.window.move_(x, y);
        self.window.show_all();
        self.window.set_keep_above(true);

        *self.start_time.borrow_mut() = Some(Instant::now());
        self.time_label.set_text("00:00");

        // Cancel previous timer if any
        if let Some(source) = self.timer_source.borrow_mut().take() {
            source.remove();
        }

        let label_clone = self.time_label.clone();
        let start_clone = self.start_time.clone();

        let source = glib::timeout_add_seconds_local(1, move || {
            if let Some(start) = *start_clone.borrow() {
                let secs = start.elapsed().as_secs();
                let mins = secs / 60;
                let rem_secs = secs % 60;
                label_clone.set_text(&format!("{:02}:{:02}", mins, rem_secs));
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        *self.timer_source.borrow_mut() = Some(source);
    }

    pub fn hide_indicator(&self) {
        if let Some(source) = self.timer_source.borrow_mut().take() {
            source.remove();
        }
        *self.start_time.borrow_mut() = None;
        self.window.hide();
    }
}
