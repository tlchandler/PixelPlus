//! Daemon runtime configuration, resolved from the environment.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

/// Which pixel output backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Real DPI output on a Raspberry Pi.
    Dpi,
    /// In-memory simulator (development, live preview only).
    Sim,
    /// No pixel output at all (Docker leader, bare Pi).
    None,
    /// Choose automatically: DPI when a supported board is detected, otherwise none.
    Auto,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    pub http_addr: SocketAddr,
    pub cluster_port: u16,
    pub output: OutputMode,
    pub tts_url: String,
    pub games_socket: PathBuf,
    /// Development mode: relaxed CORS, verbose logs.
    pub dev: bool,
}

impl Config {
    pub const DEFAULT_CLUSTER_PORT: u16 = 32320;

    pub fn from_env() -> Config {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let data_dir = env("PIXELPLUS_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/pixelplus"));
        let web_dir = env("PIXELPLUS_WEB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/share/pixelplus/web"));
        let port: u16 = env("PIXELPLUS_HTTP_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(80);
        let bind: IpAddr = env("PIXELPLUS_HTTP_BIND")
            .and_then(|b| b.parse().ok())
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let cluster_port = env("PIXELPLUS_CLUSTER_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(Self::DEFAULT_CLUSTER_PORT);
        let output = match env("PIXELPLUS_OUTPUT").as_deref() {
            Some("dpi") => OutputMode::Dpi,
            Some("sim") => OutputMode::Sim,
            Some("none") => OutputMode::None,
            _ => OutputMode::Auto,
        };
        Config {
            data_dir,
            web_dir,
            http_addr: SocketAddr::new(bind, port),
            cluster_port,
            output,
            tts_url: env("PIXELPLUS_TTS_URL").unwrap_or_else(|| "http://127.0.0.1:7081".into()),
            games_socket: env("PIXELPLUS_GAMES_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/run/pixelplus/games.sock")),
            dev: env("PIXELPLUS_DEV").is_some(),
        }
    }

    pub fn show_path(&self) -> PathBuf {
        self.data_dir.join("show.json")
    }
    pub fn node_path(&self) -> PathBuf {
        self.data_dir.join("node.json")
    }
    pub fn sequences_dir(&self) -> PathBuf {
        self.data_dir.join("sequences")
    }
    pub fn media_dir(&self) -> PathBuf {
        self.data_dir.join("media")
    }
    pub fn thumbnails_dir(&self) -> PathBuf {
        self.data_dir.join("thumbnails")
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.data_dir.join("snapshots")
    }
    pub fn games_dir(&self) -> PathBuf {
        self.data_dir.join("games")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// Create every data sub-directory.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.data_dir.clone(),
            self.sequences_dir(),
            self.media_dir(),
            self.thumbnails_dir(),
            self.snapshots_dir(),
            self.games_dir().join("roms"),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}
