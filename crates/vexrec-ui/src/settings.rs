use gtk::prelude::*;
use vexrec_core::formats::ContainerFormat;
use crate::app_state::AppState;

pub struct SettingsWindow {
    window: gtk::Window,
}

impl SettingsWindow {
    pub fn new(app_state: AppState) -> Self {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Vexrec Preferences");
        window.set_default_size(560, 420);
        window.set_position(gtk::WindowPosition::Center);
        window.set_resizable(false);
        window.style_context().add_class("settings-window");

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 16);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);
        vbox.set_margin_top(24);
        vbox.set_margin_bottom(24);

        // Header
        let title_label = gtk::Label::new(None);
        title_label.set_markup("<span font='16' weight='bold'>Vexrec Preferences</span>");
        title_label.set_halign(gtk::Align::Start);
        vbox.pack_start(&title_label, false, false, 0);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(14);
        grid.set_column_spacing(16);

        let cfg = app_state.get_config();

        // Row 0: Screenshot Directory
        let lbl_snap = gtk::Label::new(Some("Screenshots Directory:"));
        lbl_snap.set_halign(gtk::Align::Start);
        let entry_snap = gtk::Entry::new();
        entry_snap.set_text(&cfg.storage.screenshot_dir.to_string_lossy());
        entry_snap.set_hexpand(true);
        let btn_snap = gtk::Button::with_label("Browse...");

        let win_clone = window.clone();
        let entry_snap_clone = entry_snap.clone();
        btn_snap.connect_clicked(move |_| {
            let chooser = gtk::FileChooserNative::new(
                Some("Select Screenshots Folder"),
                Some(&win_clone),
                gtk::FileChooserAction::SelectFolder,
                Some("Select"),
                Some("Cancel"),
            );
            if chooser.run() == gtk::ResponseType::Accept {
                if let Some(folder) = chooser.filename() {
                    entry_snap_clone.set_text(&folder.to_string_lossy());
                }
            }
        });

        grid.attach(&lbl_snap, 0, 0, 1, 1);
        grid.attach(&entry_snap, 1, 0, 1, 1);
        grid.attach(&btn_snap, 2, 0, 1, 1);

        // Row 1: Recording Directory
        let lbl_rec = gtk::Label::new(Some("Recordings Directory:"));
        lbl_rec.set_halign(gtk::Align::Start);
        let entry_rec = gtk::Entry::new();
        entry_rec.set_text(&cfg.storage.recording_dir.to_string_lossy());
        entry_rec.set_hexpand(true);
        let btn_rec = gtk::Button::with_label("Browse...");

        let win_clone2 = window.clone();
        let entry_rec_clone = entry_rec.clone();
        btn_rec.connect_clicked(move |_| {
            let chooser = gtk::FileChooserNative::new(
                Some("Select Recordings Folder"),
                Some(&win_clone2),
                gtk::FileChooserAction::SelectFolder,
                Some("Select"),
                Some("Cancel"),
            );
            if chooser.run() == gtk::ResponseType::Accept {
                if let Some(folder) = chooser.filename() {
                    entry_rec_clone.set_text(&folder.to_string_lossy());
                }
            }
        });

        grid.attach(&lbl_rec, 0, 1, 1, 1);
        grid.attach(&entry_rec, 1, 1, 1, 1);
        grid.attach(&btn_rec, 2, 1, 1, 1);

        // Row 2: Video Framerate
        let lbl_fps = gtk::Label::new(Some("Target Framerate:"));
        lbl_fps.set_halign(gtk::Align::Start);
        let combo_fps = gtk::ComboBoxText::new();
        combo_fps.append(Some("30"), "30 FPS");
        combo_fps.append(Some("60"), "60 FPS");
        combo_fps.append(Some("120"), "120 FPS");
        combo_fps.set_active_id(Some(&cfg.recording.framerate.to_string()));
        grid.attach(&lbl_fps, 0, 2, 1, 1);
        grid.attach(&combo_fps, 1, 2, 2, 1);

        // Row 3: Video Format
        let lbl_fmt = gtk::Label::new(Some("Video Format:"));
        lbl_fmt.set_halign(gtk::Align::Start);
        let combo_fmt = gtk::ComboBoxText::new();
        combo_fmt.append(Some("mp4"), "MP4 (H.264 / AAC)");
        combo_fmt.append(Some("mkv"), "MKV (Matroska)");
        combo_fmt.append(Some("webm"), "WebM (VP9 / Opus)");
        let fmt_id = match cfg.recording.container {
            ContainerFormat::Mkv => "mkv",
            ContainerFormat::Webm => "webm",
            _ => "mp4",
        };
        combo_fmt.set_active_id(Some(fmt_id));
        grid.attach(&lbl_fmt, 0, 3, 1, 1);
        grid.attach(&combo_fmt, 1, 3, 2, 1);

        // Row 4: Show Cursor
        let lbl_cursor = gtk::Label::new(Some("Capture Mouse Cursor:"));
        lbl_cursor.set_halign(gtk::Align::Start);
        let switch_cursor = gtk::Switch::new();
        switch_cursor.set_active(cfg.recording.show_cursor);
        switch_cursor.set_halign(gtk::Align::Start);
        grid.attach(&lbl_cursor, 0, 4, 1, 1);
        grid.attach(&switch_cursor, 1, 4, 2, 1);

        // Row 5: Show Recording Frame
        let lbl_frame = gtk::Label::new(Some("Show Recording Frame:"));
        lbl_frame.set_halign(gtk::Align::Start);
        let switch_frame = gtk::Switch::new();
        switch_frame.set_active(cfg.recording.show_recording_frame);
        switch_frame.set_halign(gtk::Align::Start);
        grid.attach(&lbl_frame, 0, 5, 1, 1);
        grid.attach(&switch_frame, 1, 5, 2, 1);

        vbox.pack_start(&grid, true, true, 0);

        // Buttons row
        let hbox_btns = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox_btns.set_halign(gtk::Align::End);

        let btn_cancel = gtk::Button::with_label("Cancel");
        let btn_save = gtk::Button::with_label("Save Changes");
        btn_save.style_context().add_class("suggested-action");

        hbox_btns.pack_start(&btn_cancel, false, false, 0);
        hbox_btns.pack_start(&btn_save, false, false, 0);
        vbox.pack_start(&hbox_btns, false, false, 0);

        let win_cancel = window.clone();
        btn_cancel.connect_clicked(move |_| {
            win_cancel.hide();
        });

        let win_save = window.clone();
        let state_save = app_state.clone();
        btn_save.connect_clicked(move |_| {
            let snap_dir = std::path::PathBuf::from(entry_snap.text());
            let rec_dir = std::path::PathBuf::from(entry_rec.text());
            let fps = combo_fps
                .active_id()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(60);
            let fmt_str = combo_fmt.active_id().unwrap_or_else(|| "mp4".into());
            let container = match fmt_str.as_str() {
                "mkv" => ContainerFormat::Mkv,
                "webm" => ContainerFormat::Webm,
                _ => ContainerFormat::Mp4,
            };
            let cursor = switch_cursor.is_active();
            let frame = switch_frame.is_active();

            state_save.update_config(|cfg| {
                cfg.storage.screenshot_dir = snap_dir;
                cfg.storage.recording_dir = rec_dir;
                cfg.recording.framerate = fps;
                cfg.recording.container = container;
                cfg.recording.show_cursor = cursor;
                cfg.recording.show_recording_frame = frame;
            });

            win_save.hide();
        });

        window.add(&vbox);
        Self { window }
    }

    pub fn show(&self) {
        self.window.show_all();
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.hide();
    }
}
