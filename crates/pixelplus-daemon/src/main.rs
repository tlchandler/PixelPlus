//! `pixelplusd`: the PixelPlus show daemon.

mod api;
mod cluster;
mod config;
mod events;
mod listeners;
mod node;
mod player;
mod services;
mod state;
mod store;

use anyhow::Context;
use config::Config;
use events::EventBus;
use state::{AppInner, AppState};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    limit_malloc_arenas();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

/// glibc gives every thread that allocates concurrently its own arena (up to
/// 8 per core) and keeps what they once held: the snapshot compressor, slice
/// builds and analysis jobs run on changing blocking threads, and each new
/// arena kept ~10 MB resident even after `malloc_trim` (a 45-minute soak grew
/// the leader from 65 to 88 MB). Two arenas keep it flat on a 512 MB Pi Zero
/// 2. `MALLOC_ARENA_MAX` in the environment still wins.
fn limit_malloc_arenas() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        // SAFETY: plain allocator tuning call before any other thread exists.
        unsafe {
            libc::mallopt(libc::M_ARENA_MAX, 2);
        }
    }
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_env("PIXELPLUS_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(crate::services::logs::layer())
        .init();

    let config = Config::from_env();
    config
        .ensure_dirs()
        .with_context(|| format!("creating data directory {}", config.data_dir.display()))?;
    tracing::info!(
        "PixelPlus {} starting (data: {}, web: {})",
        env!("CARGO_PKG_VERSION"),
        config.data_dir.display(),
        config.web_dir.display()
    );

    if config.dev {
        tracing::info!("Development mode (PIXELPLUS_DEV)");
    }
    let events = EventBus::new();
    let store = store::ShowStore::load(&config.show_path(), events.clone())?;
    let identity = node::NodeIdentity::load_or_create(&config.node_path())?;
    let addr = config.http_addr;

    let state = AppState(Arc::new(AppInner {
        config,
        events,
        store,
        identity: parking_lot::RwLock::new(identity),
        sessions: Default::default(),
        started: std::time::Instant::now(),
        services: Default::default(),
    }));

    // Token for local sidecars (/run/pixelplus/local-token).
    api::security::init_local_token(&state);
    services::start_all(&state).await?;

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding HTTP port {addr}"))?;
    tracing::info!("Web interface on http://{addr}");

    // Extra listeners (listeners.rs): HTTPS for phones (F1) and the
    // public-only listener for tunnels (F14). They bind lazily and retry,
    // so a busy or privileged port never stops the daemon.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let https = tokio::spawn(listeners::serve_https(
        state.clone(),
        app.clone(),
        stop_rx.clone(),
    ));
    let public = tokio::spawn(listeners::serve_public(state.clone(), stop_rx));

    let served = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = https.await;
        let _ = public.await;
    })
    .await;
    served?;
    tracing::info!("PixelPlus stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
