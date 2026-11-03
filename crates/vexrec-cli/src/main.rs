use std::os::fd::AsRawFd;
use std::path::PathBuf;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use vexrec_core::config::{ensure_unique_path, VexrecConfig};
use vexrec_core::formats::ContainerFormat;
use vexrec_core::geometry::{Point, Rect};
use vexrec_pipeline::encoders::EncoderConfig;
use vexrec_pipeline::recorder::RecordingPipeline;
use vexrec_pipeline::snapshot::SnapshotEngine;
use vexrec_pipeline::x11_capture::X11CaptureEngine;
use vexrec_portal::PortalClient;
use vexrec_ui::annotator::{AnnotationCanvas, AnnotationTool};
use vexrec_ui::renderer::AnnotationRenderer;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "vexrec")]
#[command(bin_name = "vexrec")]
#[command(about = "Modern, minimalistic screenshot and screen recorder for Linux written in Rust", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Capture a still screenshot
    Screenshot {
        /// Interactive region selection via desktop portal
        #[arg(short, long)]
        interactive: bool,

        /// Force use of XDG Desktop Portal (even on X11)
        #[arg(long)]
        portal: bool,

        /// Direct X11 capture (bypasses portal dialog; default on X11 displays)
        #[arg(long)]
        x11: bool,

        /// Copy captured screenshot directly to clipboard
        #[arg(short, long, default_value_t = false)]
        clipboard: bool,

        /// Optional rectangular crop: x,y,width,height (e.g. 100,100,800,600)
        #[arg(long)]
        crop: Option<String>,

        /// Output file path (defaults to ~/Pictures/Vexrec/...)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Record screen with audio and optional camera overlay
    Record {
        /// Target framerate (default: 60)
        #[arg(short, long, default_value_t = 60)]
        fps: u32,

        /// Video container format (mp4, mkv, webm)
        #[arg(short, long, default_value = "mp4")]
        format: String,

        /// Enable microphone recording
        #[arg(long, default_value_t = true)]
        mic: bool,

        /// Enable desktop audio recording
        #[arg(long, default_value_t = true)]
        desktop_audio: bool,

        /// Enable webcam picture-in-picture overlay
        #[arg(long, default_value_t = false)]
        camera: bool,

        /// Output video file path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Generate a demo annotated screenshot to test the annotation renderer
    AnnotateDemo {
        /// Source image path (if omitted, captures current screen)
        #[arg(short, long)]
        input: Option<PathBuf>,

        /// Output path for annotated image
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Run as a background daemon (starts hidden with system tray and global shortcuts)
    Daemon,
    /// Trigger the instant freeze-screen interactive capture overlay
    Freeze,
    /// Stop active recording immediately
    #[command(name = "stop-recording")]
    StopRecording,
    /// Print system and multimedia capabilities diagnostics
    Doctor,
    /// Generate or install shell autocompletions (bash, zsh, fish)
    Completions {
        /// Shell to generate completions for (bash, fish, zsh, powershell, elvish)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Automatically install completion scripts into user shell config directories
        #[arg(short, long)]
        install: bool,
    },
}

fn parse_crop_rect(s: &str) -> Option<Rect> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let x = parts[0].trim().parse::<i32>().ok()?;
    let y = parts[1].trim().parse::<i32>().ok()?;
    let width = parts[2].trim().parse::<u32>().ok()?;
    let height = parts[3].trim().parse::<u32>().ok()?;
    Some(Rect::new(x, y, width, height))
}

fn expand_path(p: &std::path::Path) -> PathBuf {
    if let Ok(stripped) = p.strip_prefix("~") {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(stripped)
    } else {
        p.to_path_buf()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let cli = Cli::parse();
    let config = VexrecConfig::load().unwrap_or_default();

    let command = cli.command.unwrap_or(Commands::Freeze);

    match command {
        Commands::Completions { shell, install } => {
            let mut cmd = Cli::command();
            if install {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/usr"));

                // Bash completion
                let bash_dir = home.join(".local/share/bash-completion/completions");
                if std::fs::create_dir_all(&bash_dir).is_ok() {
                    let bash_file = bash_dir.join("vexrec");
                    if let Ok(mut f) = std::fs::File::create(&bash_file) {
                        generate(Shell::Bash, &mut cmd, "vexrec", &mut f);
                        println!("Installed Bash completion: {}", bash_file.display());
                    }
                }

                // Fish completion
                let fish_dir = home.join(".config/fish/completions");
                if std::fs::create_dir_all(&fish_dir).is_ok() {
                    let fish_file = fish_dir.join("vexrec.fish");
                    if let Ok(mut f) = std::fs::File::create(&fish_file) {
                        generate(Shell::Fish, &mut cmd, "vexrec", &mut f);
                        println!("Installed Fish completion: {}", fish_file.display());
                    }
                }

                // Zsh completion
                let zsh_dir = home.join(".local/share/zsh/site-functions");
                if std::fs::create_dir_all(&zsh_dir).is_ok() {
                    let zsh_file = zsh_dir.join("_vexrec");
                    if let Ok(mut f) = std::fs::File::create(&zsh_file) {
                        generate(Shell::Zsh, &mut cmd, "vexrec", &mut f);
                        println!("Installed Zsh completion: {}", zsh_file.display());
                    }
                }
                let zsh_custom = home.join(".zsh/completion");
                if std::fs::create_dir_all(&zsh_custom).is_ok() {
                    let zsh_file2 = zsh_custom.join("_vexrec");
                    if let Ok(mut f) = std::fs::File::create(&zsh_file2) {
                        generate(Shell::Zsh, &mut cmd, "vexrec", &mut f);
                    }
                }
                println!("All shell autocompletions installed successfully.");
            } else if let Some(sh) = shell {
                generate(sh, &mut cmd, "vexrec", &mut std::io::stdout());
            } else {
                eprintln!("Please specify a shell (bash, fish, zsh) or pass --install");
            }
        }

        Commands::Doctor => {
            println!("=== Vexrec Diagnostic Report ===");
            println!("Session type: {:?}", std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()));
            println!("Wayland display: {:?}", std::env::var("WAYLAND_DISPLAY").ok());
            println!("X11 display: {:?}", std::env::var("DISPLAY").ok());
            println!("X11 Native Capture Available: {}", X11CaptureEngine::is_available());
            println!("VA-API Hardware Video Encoder: {}", EncoderConfig::has_vaapi());
            println!("NVENC Hardware Video Encoder: {}", EncoderConfig::has_nvenc());
            println!("Config Path: {}", VexrecConfig::config_path().display());
        }

        Commands::Screenshot {
            interactive,
            portal,
            x11,
            clipboard,
            crop,
            output,
        } => {
            info!("Initiating screenshot capture");
            let crop_rect = crop.as_deref().and_then(parse_crop_rect);

            let out_path = output.map(|p| expand_path(&p)).unwrap_or_else(|| {
                let dir = expand_path(&config.storage.screenshot_dir);
                std::fs::create_dir_all(&dir).ok();
                let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
                ensure_unique_path(dir.join(format!("Vexrec_{}.png", now)))
            });

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // On X11 displays, default directly to instantaneous X11 capture unless portal or interactive selection is explicitly requested
            let use_x11 = (x11 || (!portal && !interactive)) && X11CaptureEngine::is_available();

            if use_x11 {
                println!("Capturing screen via direct X11 engine...");
                let img = X11CaptureEngine::capture_screen(crop_rect)?;

                if clipboard {
                    SnapshotEngine::copy_to_clipboard(&img)?;
                    println!("Screenshot copied directly to clipboard!");
                }

                SnapshotEngine::save_image(&img, &out_path)?;
                println!("Screenshot saved to: {}", out_path.display());
            } else {
                let portal = PortalClient::new();
                println!("Requesting screenshot via XDG Desktop Portal...");
                let result = portal.request_screenshot(interactive).await;
                let uri = match result {
                    Ok(u) => u,
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("Cancelled")
                            || err_str.contains("didn't succeed")
                            || err_str.contains("Other")
                        {
                            println!("Screenshot request cancelled by user.");
                            return Ok(());
                        }
                        return Err(e.into());
                    }
                };
                println!("Screenshot granted by portal: {}", uri);

                if let Ok(file_path) = uri.to_file_path() {
                    let mut img = image::open(&file_path)?.to_rgba8();
                    if file_path != out_path {
                        let _ = std::fs::remove_file(&file_path);
                    }

                    if let Some(r) = crop_rect {
                        img = SnapshotEngine::crop_image(&img, r)?;
                    }

                    if clipboard {
                        SnapshotEngine::copy_to_clipboard(&img)?;
                        println!("Screenshot copied directly to clipboard!");
                    }

                    SnapshotEngine::save_image(&img, &out_path)?;
                    println!("Screenshot saved to: {}", out_path.display());
                }
            }
        }

        Commands::Record {
            fps,
            format,
            mic,
            desktop_audio,
            camera,
            output,
        } => {
            let portal = PortalClient::new();
            println!("Please select the screen or window to record in the system portal dialog...");
            let streams = portal.request_screencast(config.recording.show_cursor).await?;
            let primary_stream = streams
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("No active streams received from portal"))?;

            let mut run_config = config.clone();
            run_config.recording.framerate = fps;
            run_config.audio.capture_microphone = mic;
            run_config.audio.capture_desktop = desktop_audio;
            run_config.camera.enabled = camera;
            run_config.recording.container = match format.to_lowercase().as_str() {
                "mkv" => ContainerFormat::Mkv,
                "webm" => ContainerFormat::Webm,
                _ => ContainerFormat::Mp4,
            };

            let out_path = output.map(|p| expand_path(&p)).unwrap_or_else(|| {
                let dir = expand_path(&run_config.storage.recording_dir);
                std::fs::create_dir_all(&dir).ok();
                let now = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
                ensure_unique_path(dir.join(format!("Vexrec_{}.{}", now, run_config.recording.container.extension())))
            });

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            println!("Recording will be saved to: {}", out_path.display());
            let pipeline = RecordingPipeline::new(
                primary_stream.pipewire_fd.as_raw_fd(),
                primary_stream.node_id,
                out_path.clone(),
                &run_config,
            )?;

            pipeline.start()?;
            println!("Recording started! Press Ctrl+C to stop recording...");

            tokio::signal::ctrl_c().await?;

            println!("Stopping recording and finalizing video container...");
            pipeline.stop()?;
            println!("Recording successfully finalized: {}", out_path.display());
        }

        Commands::AnnotateDemo { input, output } => {
            let base_img = if let Some(in_path) = input {
                image::open(in_path)?.to_rgba8()
            } else if X11CaptureEngine::is_available() {
                X11CaptureEngine::capture_screen(None)?
            } else {
                anyhow::bail!("No input image provided and direct X11 capture not available");
            };

            let mut canvas = AnnotationCanvas::new();
            // Add a red rectangle highlight
            canvas.add(AnnotationTool::Rectangle {
                rect: Rect::new(50, 50, 200, 120),
                color: [255, 60, 60, 255],
                stroke_width: 3.0,
                fill: None,
            });
            // Add an arrow pointing to the rectangle
            canvas.add(AnnotationTool::Arrow {
                start: Point::new(320, 220),
                end: Point::new(260, 180),
                color: [255, 60, 60, 255],
                stroke_width: 3.0,
            });
            // Add privacy blur redaction
            canvas.add(AnnotationTool::Blur {
                rect: Rect::new(60, 60, 180, 40),
                intensity: 10.0,
            });
            // Add a numbered step badge
            canvas.add(AnnotationTool::StepBadge {
                position: Point::new(45, 45),
                step: 1,
                color: [50, 150, 255, 255],
            });

            let annotated = AnnotationRenderer::render(&base_img, &canvas)?;
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            SnapshotEngine::save_image(&annotated, &output)?;
            println!("Annotated demo image successfully created at: {}", output.display());
        }

        Commands::Daemon => {
            println!("Starting Vexrec in background daemon mode (hidden)...");
            vexrec_ui::run_daemon();
        }

        Commands::Freeze => {
            match vexrec_ui::send_daemon_command("freeze").await {
                Ok(_) => {
                    println!("Triggered freeze overlay in running Vexrec instance");
                }
                Err(_) => {
                    println!("Daemon not active, starting vexrec-daemon.service...");
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "start", "vexrec-daemon.service"])
                        .status();

                    let mut triggered = false;
                    for _ in 0..40 {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        if vexrec_ui::send_daemon_command("freeze").await.is_ok() {
                            triggered = true;
                            break;
                        }
                    }

                    if triggered {
                        println!("Triggered freeze overlay in restarted Vexrec daemon");
                    } else {
                        println!("Daemon service unavailable, starting Vexrec daemon in-process...");
                        vexrec_ui::run_daemon();
                    }
                }
            }
        }

        Commands::StopRecording => {
            match vexrec_ui::send_daemon_command("stop_recording").await {
                Ok(resp) => {
                    println!("Recording stopped: {resp}");
                }
                Err(e) => {
                    eprintln!("Failed to stop recording: {e}");
                }
            }
        }
    }

    Ok(())
}
