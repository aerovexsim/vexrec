use std::os::fd::AsRawFd;
use vexrec_core::config::ensure_unique_path;
use vexrec_core::formats::ContainerFormat;
use vexrec_core::geometry::Rect;
use vexrec_pipeline::recorder::RecordingPipeline;
use vexrec_portal::PortalClient;
use serde::{Deserialize, Serialize};
use tracing::info;
use crate::app_state::{expand_path, AppState};

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

/// Sends a desktop notification whose default action opens `path` with `xdg-open`
pub fn notify_with_open_action(icon: &str, title: &str, body: &str, path: &std::path::Path) {
    let icon = icon.to_string();
    let title = title.to_string();
    let body = body.to_string();
    let path = path.to_path_buf();

    let is_video = path.extension().map_or(false, |ext| {
        let s = ext.to_string_lossy().to_ascii_lowercase();
        s == "mp4" || s == "mkv" || s == "webm" || s == "mov"
    });
    let action_label = if is_video { "Open Video" } else { "Open Screenshot" };
    let action_default = format!("default={}", action_label);
    let action_open = format!("open={}", action_label);

    std::thread::spawn(move || {
        let output = std::process::Command::new("notify-send")
            .args([
                "-a", "Vexrec",
                "-i", icon.as_str(),
                "-A", action_default.as_str(),
                "-A", action_open.as_str(),
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

pub async fn start_recording_pipeline(
    state: &AppState,
    crop: Option<Rect>,
    fps: Option<u32>,
    format: Option<String>,
    mic: Option<bool>,
    desktop_audio: Option<bool>,
) -> Result<String, String> {
    if state.is_recording() {
        return Err("A recording is already active".to_string());
    }

    let mut run_config = state.get_config();
    if let Some(f) = fps {
        run_config.recording.framerate = f;
    }
    if let Some(m) = mic {
        run_config.audio.capture_microphone = m;
    }
    if let Some(da) = desktop_audio {
        run_config.audio.capture_desktop = da;
    }
    if let Some(fmt_str) = format {
        run_config.recording.container = match fmt_str.to_lowercase().as_str() {
            "mkv" => ContainerFormat::Mkv,
            "webm" => ContainerFormat::Webm,
            _ => ContainerFormat::Mp4,
        };
    }

    let dir = expand_path(&run_config.storage.recording_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let out_path = ensure_unique_path(dir.join(format!(
        "Vexrec_{}.{}",
        now,
        run_config.recording.container.extension()
    )));

    info!("Starting recording pipeline to {}", out_path.display());

    let portal = PortalClient::new();
    let streams = portal
        .request_screencast(run_config.recording.show_cursor)
        .await
        .map_err(|e| format!("Portal screencast request failed: {e}"))?;

    let primary_stream = streams
        .into_iter()
        .next()
        .ok_or_else(|| "No active streams returned by portal".to_string())?;

    let pipeline = RecordingPipeline::new(
        primary_stream.pipewire_fd.as_raw_fd(),
        primary_stream.node_id,
        out_path.clone(),
        &run_config,
    )
    .map_err(|e| format!("Failed to create recording pipeline: {e}"))?;

    pipeline
        .start()
        .map_err(|e| format!("Failed to start recording pipeline: {e}"))?;

    *state.active_pipeline.lock().unwrap() = Some(pipeline);
    *state.active_crop.lock().unwrap() = crop;
    state.set_recording(true);

    Ok(out_path.to_string_lossy().to_string())
}

pub async fn stop_recording_internal(
    state: &AppState,
) -> Result<Option<CaptureResultDto>, String> {
    state.set_recording(false);
    *state.active_crop.lock().unwrap() = None;

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

        Ok(Some(res))
    } else {
        Ok(None)
    }
}

pub fn discard_recording_internal(state: &AppState) {
    state.set_recording(false);
    *state.active_crop.lock().unwrap() = None;
    let pipeline_opt = state.active_pipeline.lock().unwrap().take();
    if let Some(pipeline) = pipeline_opt {
        let _ = pipeline.discard();
    }
}
