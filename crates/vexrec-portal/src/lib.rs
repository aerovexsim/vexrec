use ashpd::desktop::screencast::{CursorMode, Screencast};
use ashpd::desktop::screenshot::Screenshot;
use ashpd::desktop::PersistMode;
use ashpd::url::Url;
use ashpd::WindowIdentifier;
use vexrec_core::error::{VexrecError, Result};
use std::os::fd::OwnedFd;
use tracing::info;

pub use ashpd::desktop::screencast::SourceType;
pub use ashpd::enumflags2::BitFlags;

pub struct PortalCaptureStream {
    pub pipewire_fd: OwnedFd,
    pub node_id: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

pub struct PortalClient;

impl PortalClient {
    pub fn new() -> Self {
        Self
    }

    /// Requests a screencast session via XDG Desktop Portal ScreenCast.
    /// Returns the PipeWire file descriptor and the stream node id.
    pub async fn request_screencast(
        &self,
        include_cursor: bool,
    ) -> Result<Vec<PortalCaptureStream>> {
        self.request_screencast_with_type(include_cursor, SourceType::Monitor | SourceType::Window).await
    }

    /// Requests a screencast session via XDG Desktop Portal ScreenCast with explicit SourceType.
    pub async fn request_screencast_with_type(
        &self,
        include_cursor: bool,
        source_type: impl Into<BitFlags<SourceType>>,
    ) -> Result<Vec<PortalCaptureStream>> {
        let sources: BitFlags<SourceType> = source_type.into();
        info!(?sources, "Requesting ScreenCast session via XDG Desktop Portal");
        let proxy = Screencast::new()
            .await
            .map_err(|e| VexrecError::Portal(format!("Failed to connect to ScreenCast portal: {e}")))?;

        let session = proxy
            .create_session()
            .await
            .map_err(|e| VexrecError::Portal(format!("Failed to create ScreenCast session: {e}")))?;

        let cursor_mode = if include_cursor {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        };

        proxy
            .select_sources(
                &session,
                cursor_mode,
                sources,
                true,
                None,
                PersistMode::DoNot,
            )
            .await
            .map_err(|e| VexrecError::Portal(format!("Source selection failed: {e}")))?;

        let response = proxy
            .start(&session, &WindowIdentifier::default())
            .await
            .map_err(|e| VexrecError::Portal(format!("Failed to start screencast session: {e}")))?
            .response()
            .map_err(|e| VexrecError::Portal(format!("Portal response error: {e}")))?;

        let streams = response.streams();
        if streams.is_empty() {
            return Err(VexrecError::Portal("No screen or window stream was granted by user".to_string()));
        }

        let pipewire_fd = proxy
            .open_pipe_wire_remote(&session)
            .await
            .map_err(|e| VexrecError::Portal(format!("Failed to open PipeWire remote: {e}")))?;

        let mut results = Vec::new();
        for s in streams {
            let node_id = s.pipe_wire_node_id();
            let size = s.size();
            info!(node_id, ?size, "Acquired PipeWire stream node from portal");
            let fd_clone = pipewire_fd
                .try_clone()
                .map_err(VexrecError::Io)?;
            results.push(PortalCaptureStream {
                pipewire_fd: fd_clone,
                node_id,
                width: size.map(|(w, _)| w as u32),
                height: size.map(|(_, h)| h as u32),
            });
        }

        Ok(results)
    }

    /// Ensures the XDG Desktop Portal PermissionStore grants non-interactive screenshot
    /// access to host binaries so Wayland freeze/screenshot capture operates without prompts.
    fn ensure_screenshot_permission() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            for app_id in ["", "vexrec", "net.aerovex.vexrec"] {
                let _ = std::process::Command::new("gdbus")
                    .args([
                        "call",
                        "--session",
                        "--dest",
                        "org.freedesktop.impl.portal.PermissionStore",
                        "--object-path",
                        "/org/freedesktop/impl/portal/PermissionStore",
                        "--method",
                        "org.freedesktop.impl.portal.PermissionStore.SetPermission",
                        "screenshot",
                        "true",
                        "screenshot",
                        app_id,
                        "['yes']",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
    }

    /// Requests a single screenshot via the Screenshot portal interface
    pub async fn request_screenshot(&self, interactive: bool) -> Result<Url> {
        if !interactive {
            Self::ensure_screenshot_permission();
        }
        info!(interactive, "Requesting Screenshot via XDG Desktop Portal");
        let response = Screenshot::request()
            .interactive(interactive)
            .send()
            .await
            .map_err(|e| VexrecError::Portal(format!("Screenshot request failed: {e}")))?
            .response()
            .map_err(|e| VexrecError::Portal(format!("Screenshot response error: {e}")))?;

        let uri = response.uri().clone();
        info!(uri = %uri, "Acquired screenshot from portal");
        Ok(uri)
    }
}

impl Default for PortalClient {
    fn default() -> Self {
        Self::new()
    }
}
