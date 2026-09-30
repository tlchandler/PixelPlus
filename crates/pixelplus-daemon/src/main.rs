//! `pixelplusd`: the PixelPlus show daemon.

mod api;
mod cluster;
mod config;
mod events;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("PixelPlus stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
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
