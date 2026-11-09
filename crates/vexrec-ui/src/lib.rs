pub mod annotator;
pub mod app_state;
pub mod commands;
pub mod frame;
pub mod hud;
pub mod indicator;
pub mod overlay;
pub mod renderer;
pub mod selection;
pub mod settings;

pub use annotator::{AnnotationCanvas, AnnotationTool};
pub use app_state::AppState;
pub use commands::*;
pub use frame::RecordingFrameWindow;
pub use hud::{CaptureMode, FloatingHudState};
pub use indicator::RecordingIndicatorWindow;
pub use overlay::{ActionMode, FreezeOverlayWindow, TargetMode};
pub use renderer::AnnotationRenderer;
pub use selection::SelectionManager;
pub use settings::SettingsWindow;

use gtk::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::rc::Rc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::{error, info};
use vexrec_core::geometry::Rect;

pub const SOCKET_PATH: &str = "/tmp/vexrec.sock";
pub const LEGACY_SOCKET_PATH: &str = "/tmp/luxrec.sock";

pub const DARK_CSS: &str = r#"
window {
    background-color: transparent;
}

.freeze-pill {
    background-color: rgba(24, 24, 27, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 20px;
    padding: 6px 12px;
    box-shadow: 0 16px 40px rgba(0, 0, 0, 0.6);
}

.freeze-pill button,
.freeze-pill button:backdrop {
    background-image: none;
    box-shadow: none;
    text-shadow: none;
    border-image: none;
    border: none;
    border-width: 0px;
    border-style: none;
    border-color: transparent;
    outline: none;
    outline-width: 0px;
}

.pill-group {
    background-color: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    padding: 2px;
}

.pill-separator {
    background-color: rgba(255, 255, 255, 0.15);
    margin: 4px 6px;
}

button.pill-button,
button.pill-button:backdrop {
    background: transparent;
    background-color: transparent;
    background-image: none;
    border: none;
    border-width: 0px;
    border-style: none;
    border-color: transparent;
    outline: none;
    outline-width: 0px;
    box-shadow: none;
    border-radius: 9px;
    color: #a1a1aa;
    font-size: 11px;
    font-weight: 600;
    padding: 5px 11px;
    transition: all 120ms ease;
    opacity: 1.0;
}

button.pill-button label,
button.pill-button:backdrop label {
    color: #a1a1aa;
}

button.pill-button:hover,
button.pill-button:hover:backdrop {
    color: #ffffff;
    background-color: rgba(255, 255, 255, 0.1);
}

button.pill-button:hover label,
button.pill-button:hover:backdrop label {
    color: #ffffff;
}

button.pill-button.active,
button.pill-button.active:backdrop {
    background-color: rgba(255, 255, 255, 0.22);
    color: #ffffff;
}

button.pill-button.active label,
button.pill-button.active:backdrop label {
    color: #ffffff;
}

button.action-btn-done,
button.action-btn-done:backdrop,
button.action-btn-done:focus,
button.action-btn-done:active {
    background-color: #ffffff;
    background-image: none;
    border: none;
    border-width: 0px;
    border-style: none;
    border-color: transparent;
    outline: none;
    outline-width: 0px;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35);
    border-radius: 12px;
    color: #09090b;
    font-size: 12px;
    font-weight: 700;
    padding: 6px 16px;
    opacity: 1.0;
}

button.action-btn-done label,
button.action-btn-done:backdrop label,
button.action-btn-done:focus label,
button.action-btn-done:active label {
    color: #09090b;
    font-weight: 700;
}

button.action-btn-done:hover,
button.action-btn-done:hover:backdrop {
    background-color: #f4f4f5;
    background-image: none;
    color: #000000;
    opacity: 1.0;
}

button.action-btn-done:hover label,
button.action-btn-done:hover:backdrop label {
    color: #000000;
}

button.action-btn-record,
button.action-btn-record:backdrop,
button.action-btn-record:focus,
button.action-btn-record:active {
    background-color: #ef4444;
    background-image: none;
    border: none;
    border-width: 0px;
    border-style: none;
    border-color: transparent;
    outline: none;
    outline-width: 0px;
    box-shadow: 0 4px 12px rgba(239, 68, 68, 0.35);
    border-radius: 12px;
    color: #ffffff;
    font-size: 12px;
    font-weight: 700;
    padding: 6px 16px;
    opacity: 1.0;
}

button.action-btn-record label,
button.action-btn-record:backdrop label,
button.action-btn-record:focus label,
button.action-btn-record:active label {
    color: #ffffff;
    font-weight: 700;
}

button.action-btn-record:hover,
button.action-btn-record:hover:backdrop {
    background-color: #dc2626;
    background-image: none;
    opacity: 1.0;
}

button.action-btn-record:hover label,
button.action-btn-record:hover:backdrop label {
    color: #ffffff;
}

.indicator-pill {
    background-color: rgba(24, 24, 27, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.2);
    border-radius: 14px;
    padding: 3px 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
}

.indicator-pill:hover {
    background-color: #27272a;
    border-color: rgba(255, 255, 255, 0.35);
}

.indicator-time {
    color: #ffffff;
    font-family: monospace;
    font-size: 12px;
    font-weight: 700;
}

.settings-window {
    background-color: #18181b;
    color: #f4f4f5;
}
"#;

#[derive(Clone, Debug)]
pub enum UiCommand {
    ShowFreeze,
    CancelFreeze,
    ShowRecording { crop: Option<Rect>, show_frame: bool },
    HideRecording,
}

/// Sends a command to the running background daemon over UNIX socket with a 5s timeout
pub async fn send_daemon_command(cmd: &str) -> Result<String, String> {
    use tokio::net::UnixStream as TokioUnixStream;
    let fut = async {
        let stream_res = TokioUnixStream::connect(SOCKET_PATH).await;
        let mut stream = match stream_res {
            Ok(s) => s,
            Err(_) => TokioUnixStream::connect(LEGACY_SOCKET_PATH)
                .await
                .map_err(|e| format!("Could not connect to vexrec daemon at {SOCKET_PATH}: {e}"))?,
        };

        stream
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;

        let mut reader = tokio::io::BufReader::new(stream);
        let mut response = String::new();
        let _ = reader.read_line(&mut response).await;
        let trimmed = response.trim().to_string();
        if let Some(err_msg) = trimmed.strip_prefix("ERR:") {
            return Err(err_msg.trim().to_string());
        }
        Ok(trimmed)
    };

    tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .map_err(|_| format!("Timed out waiting for vexrec daemon at {SOCKET_PATH}"))?
}

fn apply_dark_theme() {
    let provider = gtk::CssProvider::new();
    if provider.load_from_data(DARK_CSS.as_bytes()).is_ok() {
        if let Some(screen) = gdk::Screen::default() {
            gtk::StyleContext::add_provider_for_screen(
                &screen,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
    }
}

/// Spawns the synchronous background socket listener
fn spawn_native_socket_server(
    app_state: AppState,
    ui_sender: glib::Sender<UiCommand>,
) {
    let socket_path = Path::new(SOCKET_PATH);
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let legacy_path = Path::new(LEGACY_SOCKET_PATH);
    let _ = std::fs::remove_file(legacy_path);

    let listener = match UnixListener::bind(socket_path) {
        Ok(l) => {
            let _ = std::os::unix::fs::symlink(socket_path, legacy_path);
            l
        }
        Err(e) => {
            error!("Failed to bind UNIX socket at {}: {}", SOCKET_PATH, e);
            return;
        }
    };

    info!("Vexrec daemon native socket listening on {}", SOCKET_PATH);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };

            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok() {
                let cmd = line.trim();
                info!(cmd, "Received socket command");
                match cmd {
                    "freeze" => {
                        let _ = ui_sender.send(UiCommand::ShowFreeze);
                        let _ = stream.write_all(b"OK: freeze\n");
                    }
                    "stop_recording" => {
                        let state = app_state.clone();
                        let rt = tokio::runtime::Runtime::new();
                        let res = if let Ok(r) = rt {
                            r.block_on(async { stop_recording_internal(&state).await })
                        } else {
                            Err("Failed to start runtime".into())
                        };

                        let _ = ui_sender.send(UiCommand::HideRecording);

                        match res {
                            Ok(Some(r)) => {
                                let _ = stream.write_all(format!("OK: {}\n", r.file_path).as_bytes());
                            }
                            Ok(None) => {
                                let _ = stream.write_all(b"OK: no active recording\n");
                            }
                            Err(e) => {
                                let _ = stream.write_all(format!("ERR: {e}\n").as_bytes());
                            }
                        }
                    }
                    "cancel_freeze" => {
                        let _ = ui_sender.send(UiCommand::CancelFreeze);
                        let _ = stream.write_all(b"OK: freeze cancelled\n");
                    }
                    "ping" => {
                        let _ = stream.write_all(b"pong\n");
                    }
                    other => {
                        let _ = stream.write_all(format!("UNKNOWN: {other}\n").as_bytes());
                    }
                }
            }
        }
    });
}

pub fn run_app() {
    run_daemon();
}

pub fn run_daemon() {
    vexrec_pipeline::x11_capture::X11CaptureEngine::sanitize_display_env();

    gtk::init().expect("Failed to initialize native GTK for Vexrec");
    apply_dark_theme();

    let app_state = AppState::new();

    // 1. Settings Window
    let settings_win = Rc::new(SettingsWindow::new(app_state.clone()));

    // 2. Recording Frame Window
    let frame_win = Rc::new(RecordingFrameWindow::new());

    // 3. UI Command Channel (Thread-Safe Event Loop Bridge)
    let (ui_sender, ui_receiver) = glib::MainContext::channel::<UiCommand>(glib::Priority::default());

    // 4. Recording Indicator Window
    let state_ind = app_state.clone();
    let tx_ind = ui_sender.clone();

    let indicator_win = Rc::new(RecordingIndicatorWindow::new(move || {
        let state = state_ind.clone();
        let tx = tx_ind.clone();

        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                let _ = rt.block_on(async { stop_recording_internal(&state).await });
            }
            let _ = tx.send(UiCommand::HideRecording);
        });
    }));

    // 5. Freeze Overlay Window
    let state_ov = app_state.clone();
    let tx_ov = ui_sender.clone();
    let settings_ov = settings_win.clone();

    let overlay_win = FreezeOverlayWindow::new(
        app_state.clone(),
        move |crop, mic, audio| {
            let state = state_ov.clone();
            let tx = tx_ov.clone();
            let show_frame = state.get_config().recording.show_recording_frame;

            std::thread::spawn(move || {
                let res = if let Ok(rt) = tokio::runtime::Runtime::new() {
                    rt.block_on(async {
                        start_recording_pipeline(
                            &state,
                            crop,
                            Some(60),
                            Some("mp4".into()),
                            Some(mic),
                            Some(audio),
                        )
                        .await
                    })
                } else {
                    Err("Runtime creation failed".into())
                };

                if res.is_ok() {
                    let _ = tx.send(UiCommand::ShowRecording { crop, show_frame });
                }
            });
        },
        move || {
            settings_ov.show();
        },
    );

    // Attach receiver on main thread
    let ov_rcv = overlay_win.clone();
    let ind_rcv = indicator_win.clone();
    let frame_rcv = frame_win.clone();
    let settings_rcv = settings_win.clone();

    ui_receiver.attach(None, move |cmd| {
        match cmd {
            UiCommand::ShowFreeze => {
                settings_rcv.hide();
                ov_rcv.show_freeze();
            }
            UiCommand::CancelFreeze => {
                ov_rcv.cancel();
            }
            UiCommand::ShowRecording { crop, show_frame } => {
                ind_rcv.show_indicator();
                if show_frame {
                    frame_rcv.show_frame(crop);
                }
            }
            UiCommand::HideRecording => {
                ind_rcv.hide_indicator();
                frame_rcv.hide_frame();
            }
        }
        glib::ControlFlow::Continue
    });

    // 6. Start Background Socket Listener
    spawn_native_socket_server(app_state, ui_sender);

    gtk::main();
}
