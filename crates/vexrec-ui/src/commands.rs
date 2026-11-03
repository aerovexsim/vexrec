use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Instant;
use base64::Engine;
use image::ImageEncoder;
use vexrec_core::config::{ensure_unique_path, VexrecConfig};
use vexrec_core::formats::ContainerFormat;
use vexrec_core::geometry::Rect;
use vexrec_pipeline::encoders::EncoderConfig;
use vexrec_pipeline::recorder::RecordingPipeline;
use vexrec_pipeline::snapshot::SnapshotEngine;
use vexrec_pipeline::x11_capture::X11CaptureEngine;
use vexrec_portal::{PortalClient, SourceType};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemStatusDto {
    pub session_type: String,
    pub has_vaapi: bool,
    pub has_nvenc: bool,
    pub x11_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResultDto {
    pub file_path: String,
    pub width: u32,
    pub height: u32,
    pub file_size_bytes: u64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreezeDataDto {
    pub seq: u64,
    pub image_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    pub width: u32,
    pub height: u32,
    pub last_mode: Option<String>,
    pub last_region: Option<Rect>,
}

pub struct AppRecordingState {
    pub active_pipeline: Mutex<Option<RecordingPipeline>>,
    pub freeze_data: Mutex<Option<FreezeDataDto>>,
    /// Monotonic frame counter used to ignore stale freeze frames.
    pub freeze_seq: AtomicU64,
    /// Highest frame sequence acknowledged by the overlay webview.
    pub freeze_ack: Mutex<u64>,
    pub freeze_ack_cv: Condvar,
    pub is_recording: AtomicBool,
    pub recording_start: Mutex<Option<Instant>>,
}

impl Default for AppRecordingState {
    fn default() -> Self {
        Self {
            active_pipeline: Mutex::new(None),
            freeze_data: Mutex::new(None),
            freeze_seq: AtomicU64::new(0),
            freeze_ack: Mutex::new(0),
            freeze_ack_cv: Condvar::new(),
            is_recording: AtomicBool::new(false),
            recording_start: Mutex::new(None),
        }
    }
}

pub fn expand_path(p: &std::path::Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    p.to_path_buf()
}

/// Sends a desktop notification whose default action (clicking the notification body or button)
/// opens `path` with the system default application for its file type via `xdg-open`.
/// The notification is handled on a detached thread so callers are never blocked.
pub fn notify_with_open_action(icon: &str, title: &str, body: &str, path: &std::path::Path) {
    let icon = icon.to_string();
    let title = title.to_string();
    let body = body.to_string();
    let path = path.to_path_buf();

    std::thread::spawn(move || {
        let output = std::process::Command::new("notify-send")
            .args([
                "-a", "Vexrec",
                "-i", icon.as_str(),
                "-A", "default=Open Video",
                "-A", "open=Open Video",
                "-t", "10000",
                "--wait",
                title.as_str(),
                body.as_str(),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
            if !s.is_empty() && (s == "default" || s == "open" || s == "0" || s == "1") {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    });
}

pub fn create_red_circle_icon() -> tauri::image::Image<'static> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let center = (SIZE as f32 - 1.0) / 2.0;
    let radius = 11.0;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius - 0.5 {
                rgba.extend_from_slice(&[239, 68, 68, 255]);
            } else if dist <= radius + 0.5 {
                let alpha = ((radius + 0.5 - dist) * 255.0).clamp(0.0, 255.0) as u8;
                rgba.extend_from_slice(&[239, 68, 68, alpha]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

pub fn show_recording_indicator(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("recording_indicator") {
        let monitor = win.current_monitor().ok().flatten();
        let (screen_w, _) = monitor
            .map(|m| (m.size().width, m.size().height))
            .unwrap_or((1920, 1080));

        let width = 100;
        let height = 28;
        // Position at top-right corner of screen: x = screen_w - 120, y = 38 (just below GNOME top bar)
        let x = (screen_w as i32) - 120;
        let y = 38;

        let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
        let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width,
            height,
        }));
        let _ = win.set_always_on_top(true);
        let _ = win.set_skip_taskbar(true);
        let _ = win.show();
        let _ = app.emit("vexrec://recording-started", ());
        let _ = app.emit("luxrec://recording-started", ());
    }
    Ok(())
}

pub fn hide_recording_indicator(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("recording_indicator") {
        let _ = win.hide();
    }
}

pub fn show_recording_tray(app: &tauri::AppHandle) -> Result<(), String> {
    let icon = create_red_circle_icon();
    let app_handle = app.clone();

    // If tray already exists on this app handle, simply make it visible and update title/icon
    if let Some(tray) = app.tray_by_id("recording_tray") {
        let _ = tray.set_icon(Some(icon));
        let _ = tray.set_visible(true);
        let _ = tray.set_title(Some("00:00"));
        let _ = tray.set_tooltip(Some("Recording: 00:00 - Click to stop"));
        return Ok(());
    }

    let stop_item = tauri::menu::MenuItem::with_id(app, "stop", "Stop Recording", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let discard_item = tauri::menu::MenuItem::with_id(app, "discard", "Discard Recording", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let menu = tauri::menu::Menu::with_items(app, &[&stop_item, &discard_item])
        .map_err(|e| e.to_string())?;

    tauri::tray::TrayIconBuilder::with_id("recording_tray")
        .icon(icon)
        .title("00:00")
        .tooltip("Recording: 00:00 - Click to stop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            if id == "stop" {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<AppRecordingState>() {
                        let _ = stop_recording_internal(&app, &state).await;
                    }
                });
            } else if id == "discard" {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<AppRecordingState>() {
                        discard_recording_internal(&app, &state).await;
                    }
                });
            }
        })
        .on_tray_icon_event(move |_tray, event| {
            if let tauri::tray::TrayIconEvent::Click { button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<AppRecordingState>() {
                        let _ = stop_recording_internal(&app, &state).await;
                    }
                });
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn hide_recording_tray(app: &tauri::AppHandle) {
    if let Some(tray) = app.tray_by_id("recording_tray") {
        let _ = tray.set_visible(false);
    }
}

pub fn show_recording_frame(
    app: &tauri::AppHandle,
    crop: Option<Rect>,
) -> Result<(), String> {
    let config = VexrecConfig::load().unwrap_or_default();
    if !config.recording.show_recording_frame {
        return Ok(());
    }

    if let Some(frame_win) = app.get_webview_window("recording_frame") {
        const BORDER_THICKNESS: i32 = 3;

        // Position window outside the crop region so the border is never captured in video
        let (pos_x, pos_y, size_w, size_h) = match crop {
            Some(r) => {
                let x = (r.x - BORDER_THICKNESS).max(0);
                let y = (r.y - BORDER_THICKNESS).max(0);
                let w = (r.width + 2 * BORDER_THICKNESS as u32) as u32;
                let h = (r.height + 2 * BORDER_THICKNESS as u32) as u32;
                (x, y, w, h)
            }
            None => {
                let monitor = frame_win.current_monitor().ok().flatten();
                let (w, h) = monitor
                    .map(|m| (m.size().width, m.size().height))
                    .unwrap_or((1920, 1080));
                (0, 0, w, h)
            }
        };

        let _ = frame_win.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: pos_x,
            y: pos_y,
        }));
        let _ = frame_win.set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: size_w,
            height: size_h,
        }));

        let _ = frame_win.set_always_on_top(true);
        let _ = frame_win.set_skip_taskbar(true);
        let _ = frame_win.show();
        // Window passes all mouse clicks and gestures straight through (not interactable)
        let _ = frame_win.set_ignore_cursor_events(true);
    }
    Ok(())
}

pub fn hide_recording_frame(app: &tauri::AppHandle) {
    if let Some(frame_win) = app.get_webview_window("recording_frame") {
        let _ = frame_win.hide();
    }
}

pub async fn discard_recording_internal(
    app: &tauri::AppHandle,
    state: &AppRecordingState,
) {
    state.is_recording.store(false, Ordering::SeqCst);
    *state.recording_start.lock().unwrap() = None;
    hide_recording_tray(app);
    hide_recording_indicator(app);
    hide_recording_frame(app);
    let pipeline_opt = state.active_pipeline.lock().unwrap().take();
    if let Some(pipeline) = pipeline_opt {
        let _ = pipeline.discard();
    }
}

pub async fn stop_recording_internal(
    app: &tauri::AppHandle,
    state: &AppRecordingState,
) -> Result<Option<CaptureResultDto>, String> {
    state.is_recording.store(false, Ordering::SeqCst);
    *state.recording_start.lock().unwrap() = None;
    hide_recording_tray(app);
    hide_recording_indicator(app);
    hide_recording_frame(app);

    let pipeline_opt = state.active_pipeline.lock().unwrap().take();
    if let Some(pipeline) = pipeline_opt {
        let out_path = pipeline.output_path().clone();
        pipeline.stop().map_err(|e| e.to_string())?;

        let meta = std::fs::metadata(&out_path).ok();
        let file_size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

        notify_with_open_action(
            "video-x-generic",
            "Recording Saved",
            &format!("Video saved to {}", out_path.display()),
            &out_path,
        );

        let res = CaptureResultDto {
            file_path: out_path.to_string_lossy().to_string(),
            width: 1920,
            height: 1080,
            file_size_bytes: file_size,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        let _ = app.emit("vexrec://recording-stopped", &res);
        let _ = app.emit("luxrec://recording-stopped", &res);
        Ok(Some(res))
    } else {
        Ok(None)
    }
}

pub async fn trigger_freeze(
    app: &tauri::AppHandle,
    state: &AppRecordingState,
) -> Result<(), String> {
    // Ensure any existing freeze overlay is synchronously unmapped first
    hide_freeze_overlay(app);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let x11_native = X11CaptureEngine::is_available();
    let img = if x11_native {
        match X11CaptureEngine::capture_screen(None) {
            Ok(captured) => captured,
            Err(_) => {
                let portal = PortalClient::new();
                let uri = portal.request_screenshot(false).await.map_err(|e| e.to_string())?;
                let file_path = uri.to_file_path().map_err(|_| "Invalid file URI".to_string())?;
                let loaded = image::open(&file_path).map_err(|e| e.to_string())?.to_rgba8();
                let _ = std::fs::remove_file(&file_path);
                loaded
            }
        }
    } else {
        let portal = PortalClient::new();
        let uri = portal.request_screenshot(false).await.map_err(|e| e.to_string())?;
        let file_path = uri.to_file_path().map_err(|_| "Invalid file URI".to_string())?;
        let loaded = image::open(&file_path).map_err(|e| e.to_string())?.to_rgba8();
        let _ = std::fs::remove_file(&file_path);
        loaded
    };
    let (width, height) = img.dimensions();

    let freeze_path = PathBuf::from("/tmp/vexrec_freeze.png");
    SnapshotEngine::save_image(&img, &freeze_path).map_err(|e| e.to_string())?;

    let mut png_bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);
    encoder
        .write_image(&img, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let image_url = Some(format!("data:image/png;base64,{}", b64));

    let config = VexrecConfig::load().unwrap_or_default();
    let seq = state.freeze_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let freeze_dto = FreezeDataDto {
        seq,
        image_path: freeze_path.to_string_lossy().to_string(),
        image_url,
        width,
        height,
        last_mode: config.screenshot.last_mode,
        last_region: config.screenshot.last_region,
    };

    *state.freeze_data.lock().unwrap() = Some(freeze_dto.clone());

    let _ = app.emit("vexrec://freeze-ready", &freeze_dto);
    let _ = app.emit("luxrec://freeze-ready", &freeze_dto);

    // Wait (bounded) until the overlay webview has decoded and committed this exact frame.
    // Mapping the window earlier shows the previous frame or an unpainted surface until the
    // user interacts with it.
    tokio::task::block_in_place(|| {
        let acked = state.freeze_ack.lock().unwrap();
        let _ = state
            .freeze_ack_cv
            .wait_timeout_while(acked, std::time::Duration::from_millis(1200), |a| *a < seq)
            .unwrap();
    });

    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_fullscreen(true);
        let _ = overlay.set_always_on_top(true);
        let _ = overlay.set_skip_taskbar(true);
        overlay.show().map_err(|e| e.to_string())?;
        overlay.set_focus().map_err(|e| e.to_string())?;
    }

    if x11_native {
        // Configure ONLY the full-screen overlay X11 window to UTILITY/ABOVE and request focus.
        // The window must be viewable before the window manager honours _NET_ACTIVE_WINDOW, so
        // wait for the map to complete and re-assert focus a few times to win any focus races.
        let min_w = width.min(u16::MAX as u32) as u16;
        let min_h = height.min(u16::MAX as u32) as u16;
        let mut overlay_win = None;
        for _ in 0..50 {
            if let Ok(Some(w)) = X11CaptureEngine::find_overlay_window(std::process::id(), min_w, min_h) {
                overlay_win = Some(w);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        if let Some(win) = overlay_win {
            let _ = X11CaptureEngine::wait_until_viewable(win, std::time::Duration::from_millis(500));
            let _ = X11CaptureEngine::configure_dock_overlay(win);
            for attempt in 0..12u64 {
                let _ = X11CaptureEngine::activate_and_grab_focus(win);
                std::thread::sleep(std::time::Duration::from_millis(30 + attempt * 10));
                if X11CaptureEngine::active_window() == Some(win) {
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Hides the freeze overlay and discards the stored frame so a stale image can never be
/// presented the next time the overlay is shown. The webview is told to drop its copy too.
pub fn hide_freeze_overlay(app: &tauri::AppHandle) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
    if X11CaptureEngine::is_available() {
        if let Ok(Some(win)) = X11CaptureEngine::find_overlay_window(std::process::id(), 500, 500) {
            let _ = X11CaptureEngine::unmap_window(win);
        }
    }
    if let Some(state) = app.try_state::<AppRecordingState>() {
        *state.freeze_data.lock().unwrap() = None;
    }
    let _ = app.emit("vexrec://freeze-hidden", ());
}

#[tauri::command]
pub async fn get_system_status() -> Result<SystemStatusDto, String> {
    Ok(SystemStatusDto {
        session_type: std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()),
        has_vaapi: EncoderConfig::has_vaapi(),
        has_nvenc: EncoderConfig::has_nvenc(),
        x11_available: X11CaptureEngine::is_available(),
    })
}

#[tauri::command]
pub async fn get_config() -> Result<VexrecConfig, String> {
    VexrecConfig::load().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_config(config: VexrecConfig) -> Result<(), String> {
    let path = VexrecConfig::config_path();
    config.save(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_autostart() -> Result<bool, String> {
    let autostart_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("autostart");
    let desktop_file = autostart_dir.join("vexrec.desktop");
    Ok(desktop_file.exists())
}

#[tauri::command]
pub async fn set_autostart(enabled: bool) -> Result<(), String> {
    let autostart_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("autostart");
    let desktop_file = autostart_dir.join("vexrec.desktop");
    let legacy_desktop = autostart_dir.join("luxrec.desktop");
    let _ = std::fs::remove_file(legacy_desktop);

    if enabled {
        let _ = std::fs::create_dir_all(&autostart_dir);
        let content = "[Desktop Entry]\nType=Application\nName=Vexrec\nExec=vexrec daemon\nIcon=vexrec\nTerminal=false\nCategories=Utility;AudioVideo;Recorder;\nX-GNOME-Autostart-enabled=true\n";
        std::fs::write(&desktop_file, content).map_err(|e| e.to_string())?;
    } else if desktop_file.exists() {
        let _ = std::fs::remove_file(&desktop_file);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_system_screenshot_status() -> Result<bool, String> {
    let output = std::process::Command::new("gsettings")
        .args([
            "get",
            "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-screenshot/",
            "binding",
        ])
        .output();

    if let Ok(out) = output {
        let s = String::from_utf8_lossy(&out.stdout);
        return Ok(s.contains("Print"));
    }
    Ok(false)
}

#[tauri::command]
pub async fn replace_system_screenshot(enabled: bool) -> Result<(), String> {
    if enabled {
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-screenshot/", "name", "Vexrec Interactive Screenshot"])
            .status();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-screenshot/", "command", "vexrec freeze"])
            .status();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-screenshot/", "binding", "Print"])
            .status();

        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-area/", "name", "Vexrec Stop Recording"])
            .status();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-area/", "command", "vexrec stop-recording"])
            .status();
        let _ = std::process::Command::new("gsettings")
            .args(["set", "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-area/", "binding", "<Shift>Print"])
            .status();

        let list_out = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.settings-daemon.plugins.media-keys", "custom-keybindings"])
            .output();
        if let Ok(out) = list_out {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut paths: Vec<String> = s
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .map(|p| p.trim().trim_matches('\'').to_string())
                .filter(|p| !p.is_empty() && !p.contains("luxrec"))
                .collect();

            let p1 = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-screenshot/".to_string();
            let p2 = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/vexrec-area/".to_string();

            if !paths.contains(&p1) {
                paths.push(p1);
            }
            if !paths.contains(&p2) {
                paths.push(p2);
            }

            let formatted = format!("[{}]", paths.iter().map(|p| format!("'{}'", p)).collect::<Vec<_>>().join(", "));
            let _ = std::process::Command::new("gsettings")
                .args(["set", "org.gnome.settings-daemon.plugins.media-keys", "custom-keybindings", &formatted])
                .status();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_window_position(window: tauri::WebviewWindow) -> Result<[i32; 2], String> {
    let pos = window.outer_position().map_err(|e| e.to_string())?;
    Ok([pos.x, pos.y])
}

#[tauri::command]
pub async fn set_window_position(window: tauri::WebviewWindow, x: i32, y: i32) -> Result<(), String> {
    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(settings) = app.get_webview_window("settings") {
        settings.show().map_err(|e| e.to_string())?;
        settings.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_freeze_data(
    state: tauri::State<'_, AppRecordingState>,
) -> Result<Option<FreezeDataDto>, String> {
    Ok(state.freeze_data.lock().unwrap().clone())
}

/// Called by the overlay webview once the frame identified by `seq` has been decoded and
/// committed to the DOM. The backend waits for this acknowledgement before mapping the window,
/// so the user never sees a previous frame or an unpainted overlay.
#[tauri::command]
pub async fn freeze_frame_ready(
    state: tauri::State<'_, AppRecordingState>,
    seq: u64,
) -> Result<(), String> {
    let mut acked = state.freeze_ack.lock().unwrap();
    if seq > *acked {
        *acked = seq;
    }
    state.freeze_ack_cv.notify_all();
    Ok(())
}

#[tauri::command]
pub async fn cancel_freeze(app: tauri::AppHandle) -> Result<(), String> {
    hide_freeze_overlay(&app);
    Ok(())
}

#[tauri::command]
pub async fn confirm_freeze_capture(
    app: tauri::AppHandle,
    _state: tauri::State<'_, AppRecordingState>,
    crop: Option<Rect>,
    copy_to_clipboard: Option<bool>,
    custom_path: Option<String>,
) -> Result<CaptureResultDto, String> {
    hide_freeze_overlay(&app);

    let base_img = if std::path::Path::new("/tmp/vexrec_freeze.png").exists() {
        image::open("/tmp/vexrec_freeze.png")
    } else {
        image::open("/tmp/luxrec_freeze.png")
    }
    .map_err(|e| e.to_string())?
    .to_rgba8();

    let cropped = if let Some(r) = crop {
        SnapshotEngine::crop_image(&base_img, r).map_err(|e| e.to_string())?
    } else {
        base_img
    };

    let config = VexrecConfig::load().unwrap_or_default();
    let save_dir = expand_path(&config.storage.screenshot_dir);
    let out_path = if let Some(ref p) = custom_path {
        if p == "PROMPT_SAVE" || p.is_empty() {
            let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
            ensure_unique_path(save_dir.join(format!("Vexrec_{}.png", now)))
        } else {
            expand_path(&PathBuf::from(p))
        }
    } else {
        let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        ensure_unique_path(save_dir.join(format!("Vexrec_{}.png", now)))
    };

    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    SnapshotEngine::save_image(&cropped, &out_path).map_err(|e| e.to_string())?;

    if copy_to_clipboard.unwrap_or(true) {
        let _ = SnapshotEngine::copy_to_clipboard(&cropped);
    }

    let mut updated_config = config.clone();
    if let Some(r) = crop {
        updated_config.screenshot.last_region = Some(r);
        updated_config.screenshot.last_mode = Some("region".to_string());
    } else {
        updated_config.screenshot.last_mode = Some("fullscreen".to_string());
    }
    let _ = updated_config.save(&VexrecConfig::config_path());

    let meta = std::fs::metadata(&out_path).map_err(|e| e.to_string())?;
    let (w, h) = cropped.dimensions();

    notify_with_open_action(
        "camera-photo",
        "Screenshot Saved",
        &format!("Saved to {}\nCopied to clipboard", out_path.display()),
        &out_path,
    );

    Ok(CaptureResultDto {
        file_path: out_path.to_string_lossy().to_string(),
        width: w,
        height: h,
        file_size_bytes: meta.len(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

#[tauri::command]
pub async fn start_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppRecordingState>,
    _target: Option<String>,
    crop: Option<Rect>,
    fps: Option<u32>,
    format: Option<String>,
    mic: Option<bool>,
    desktop_audio: Option<bool>,
) -> Result<(), String> {
    let mut config = VexrecConfig::load().unwrap_or_default();
    if let Some(f) = fps {
        config.recording.framerate = f;
    }
    if let Some(m) = mic {
        config.audio.capture_microphone = m;
    }
    if let Some(d) = desktop_audio {
        config.audio.capture_desktop = d;
    }
    if let Some(fmt) = format {
        config.recording.container = match fmt.to_lowercase().as_str() {
            "mkv" => ContainerFormat::Mkv,
            "webm" => ContainerFormat::Webm,
            "gif" => ContainerFormat::Gif,
            _ => ContainerFormat::Mp4,
        };
    }

    let out_dir = expand_path(&config.storage.recording_dir);
    let _ = std::fs::create_dir_all(&out_dir);
    let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let out_path = ensure_unique_path(out_dir.join(format!("Vexrec_{}.{}", now, config.recording.container.extension())));

    // Synchronously unmap and hide freeze overlay before starting pipeline
    hide_freeze_overlay(&app);
    // Allow window manager / compositor a brief moment to repaint the live underlying desktop
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    let pipeline = if X11CaptureEngine::is_available() {
        RecordingPipeline::new_x11(crop, out_path, &config).map_err(|e| e.to_string())?
    } else {
        let portal = PortalClient::new();
        let source_type = if _target.as_deref() == Some("window") {
            SourceType::Window
        } else {
            SourceType::Monitor
        };
        let streams = portal
            .request_screencast_with_type(config.recording.show_cursor, source_type)
            .await
            .map_err(|e| e.to_string())?;
        let primary_stream = streams
            .into_iter()
            .next()
            .ok_or_else(|| "No active screen stream granted by portal".to_string())?;
        use std::os::fd::AsRawFd;
        RecordingPipeline::new(
            primary_stream.pipewire_fd.as_raw_fd(),
            primary_stream.node_id,
            out_path,
            &config,
        ).map_err(|e| e.to_string())?
    };

    pipeline.start().map_err(|e| e.to_string())?;
    *state.active_pipeline.lock().unwrap() = Some(pipeline);

    // Record start time & active status
    state.is_recording.store(true, Ordering::SeqCst);
    *state.recording_start.lock().unwrap() = Some(Instant::now());

    // Show top-right recording indicator window with live timer
    let _ = show_recording_indicator(&app);

    // Show top-right reddish circle tray icon with initial duration
    let _ = show_recording_tray(&app);

    // Spawn ticker to update tray duration every second from actual start time
    let app_ticker = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let (is_active, start_instant) = {
                if let Some(st) = app_ticker.try_state::<AppRecordingState>() {
                    let active = st.is_recording.load(Ordering::SeqCst);
                    let start = *st.recording_start.lock().unwrap();
                    (active, start)
                } else {
                    (false, None)
                }
            };
            if !is_active {
                break;
            }
            if let Some(start) = start_instant {
                let elapsed = start.elapsed().as_secs();
                let time_str = if elapsed < 3600 {
                    format!("{:02}:{:02}", elapsed / 60, elapsed % 60)
                } else {
                    format!("{:02}:{:02}:{:02}", elapsed / 3600, (elapsed % 3600) / 60, elapsed % 60)
                };
                if let Some(tray) = app_ticker.tray_by_id("recording_tray") {
                    let _ = tray.set_title(Some(&time_str));
                    let _ = tray.set_tooltip(Some(&format!("Recording: {} - Click to stop", time_str)));
                } else {
                    break;
                }
            }
        }
    });

    // Show optional non-interactable border frame around the recording area
    show_recording_frame(&app, crop)?;

    Ok(())
}

#[tauri::command]
pub async fn stop_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppRecordingState>,
) -> Result<Option<CaptureResultDto>, String> {
    stop_recording_internal(&app, &state).await
}

#[tauri::command]
pub async fn pause_recording(
    state: tauri::State<'_, AppRecordingState>,
) -> Result<(), String> {
    if let Some(pipeline) = state.active_pipeline.lock().unwrap().as_ref() {
        pipeline.pause().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn resume_recording(
    state: tauri::State<'_, AppRecordingState>,
) -> Result<(), String> {
    if let Some(pipeline) = state.active_pipeline.lock().unwrap().as_ref() {
        pipeline.resume().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn discard_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppRecordingState>,
) -> Result<(), String> {
    discard_recording_internal(&app, &state).await;
    Ok(())
}

#[tauri::command]
pub async fn capture_screenshot(
    crop: Option<Rect>,
    direct_x11: bool,
) -> Result<CaptureResultDto, String> {
    let config = VexrecConfig::load().unwrap_or_default();
    let dir = expand_path(&config.storage.screenshot_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let out_path = ensure_unique_path(dir.join(format!("Vexrec_{}.png", now)));

    let img = if direct_x11 && X11CaptureEngine::is_available() {
        X11CaptureEngine::capture_screen(crop).map_err(|e| e.to_string())?
    } else {
        let portal = PortalClient::new();
        let uri = portal.request_screenshot(false).await.map_err(|e| e.to_string())?;
        let file_path = uri.to_file_path().map_err(|_| "Invalid file URI".to_string())?;
        let mut loaded = image::open(&file_path).map_err(|e| e.to_string())?.to_rgba8();
        if file_path != out_path {
            let _ = std::fs::remove_file(&file_path);
        }
        if let Some(r) = crop {
            loaded = SnapshotEngine::crop_image(&loaded, r).map_err(|e| e.to_string())?;
        }
        loaded
    };

    let (width, height) = img.dimensions();
    SnapshotEngine::save_image(&img, &out_path).map_err(|e| e.to_string())?;

    let meta = std::fs::metadata(&out_path).map_err(|e| e.to_string())?;

    Ok(CaptureResultDto {
        file_path: out_path.to_string_lossy().to_string(),
        width,
        height,
        file_size_bytes: meta.len(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

#[tauri::command]
pub async fn copy_image_to_clipboard(path: String) -> Result<(), String> {
    let img = image::open(&path).map_err(|e| e.to_string())?.to_rgba8();
    SnapshotEngine::copy_to_clipboard(&img).map_err(|e| e.to_string())
}
