use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use vexrec_core::config::VexrecConfig;
use vexrec_core::geometry::Rect;
use vexrec_pipeline::recorder::RecordingPipeline;

#[derive(Clone)]
pub struct AppState {
    pub active_pipeline: Arc<Mutex<Option<RecordingPipeline>>>,
    pub is_recording: Arc<AtomicBool>,
    pub recording_start: Arc<Mutex<Option<Instant>>>,
    pub active_crop: Arc<Mutex<Option<Rect>>>,
    pub config: Arc<Mutex<VexrecConfig>>,
}

impl Default for AppState {
    fn default() -> Self {
        let config = VexrecConfig::load().unwrap_or_default();
        Self {
            active_pipeline: Arc::new(Mutex::new(None)),
            is_recording: Arc::new(AtomicBool::new(false)),
            recording_start: Arc::new(Mutex::new(None)),
            active_crop: Arc::new(Mutex::new(None)),
            config: Arc::new(Mutex::new(config)),
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn set_recording(&self, val: bool) {
        self.is_recording.store(val, Ordering::SeqCst);
        if val {
            *self.recording_start.lock().unwrap() = Some(Instant::now());
        } else {
            *self.recording_start.lock().unwrap() = None;
        }
    }

    pub fn elapsed_seconds(&self) -> u64 {
        if let Some(start) = *self.recording_start.lock().unwrap() {
            start.elapsed().as_secs()
        } else {
            0
        }
    }

    pub fn get_config(&self) -> VexrecConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn update_config<F>(&self, f: F)
    where
        F: FnOnce(&mut VexrecConfig),
    {
        let mut cfg = self.config.lock().unwrap();
        f(&mut cfg);
        let path = VexrecConfig::config_path();
        let _ = cfg.save(&path);
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
