//! Triggers (`settings.triggers`): physical buttons on free GPIOs and HTTP
//! triggers (`POST /triggers/:id/fire`), mapped to player actions.

use crate::api::{ApiError, ApiResult};
use crate::player::{PlayRequest, PlayerCmd};
use crate::state::AppState;
use pixelplus_core::model::{Trigger, TriggerAction, TriggerActionType, TriggerKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn player(state: &AppState) -> ApiResult<&crate::player::PlayerHandle> {
    state.services.player.get().ok_or_else(|| {
        ApiError::unavailable("The player is still starting. Try again in a moment.")
    })
}

/// Carry out a trigger action. Returns a short description of what happened.
pub async fn run_action(state: &AppState, action: &TriggerAction) -> ApiResult<String> {
    let show = state.store.get();
    let r = action.r#ref.as_deref().unwrap_or("");
    let p = player(state)?;
    let empty = PlayRequest {
        playlist_id: None,
        sequence_id: None,
        dj_clip_id: None,
        effect_id: None,
        media_id: None,
        start_index: None,
    };
    match action.kind {
        TriggerActionType::Stop => {
            p.send(PlayerCmd::Stop { fade: true }).await?;
            Ok("Stopped the show".into())
        }
        TriggerActionType::PlayPlaylist => {
            let pl = show
                .playlist(r)
                .or_else(|| {
                    show.playlists
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(r))
                })
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "This trigger's playlist no longer exists. Edit the trigger.",
                    )
                })?;
            p.play(PlayRequest {
                playlist_id: Some(pl.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Playing {}", pl.name))
        }
        TriggerActionType::PlaySequence => {
            let s = show.sequence(r).ok_or_else(|| {
                ApiError::bad_request("This trigger's sequence no longer exists. Edit the trigger.")
            })?;
            p.play(PlayRequest {
                sequence_id: Some(s.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Playing {}", s.name))
        }
        TriggerActionType::Effect => {
            let e = show.effect(r).ok_or_else(|| {
                ApiError::bad_request("This trigger's look no longer exists. Edit the trigger.")
            })?;
            p.play(PlayRequest {
                effect_id: Some(e.id.clone()),
                ..empty
            })
            .await?;
            Ok(format!("Showing {}", e.name))
        }
    }
}

pub async fn fire(state: &AppState, id: &str) -> ApiResult<String> {
    let show = state.store.get();
    let t: Trigger = show
        .settings
        .triggers
        .iter()
        .find(|t| t.id == id || t.name.eq_ignore_ascii_case(id))
        .cloned()
        .ok_or_else(|| ApiError::not_found("That trigger"))?;
    let msg = run_action(state, &t.action).await?;
    tracing::info!("Trigger \"{}\": {msg}", t.name);
    Ok(msg)
}

fn gpio_pins(state: &AppState) -> Vec<u8> {
    let mut pins: Vec<u8> = state
        .store
        .get()
        .settings
        .triggers
        .iter()
        .filter(|t| t.kind == TriggerKind::Gpio)
        .filter_map(|t| t.gpio)
        .collect();
    pins.sort_unstable();
    pins.dedup();
    pins
}

/// Watch GPIO buttons; re-open them whenever the trigger list changes.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let mut changes = state.store.subscribe();
        let mut current: Vec<u8> = Vec::new();
        let mut stop: Option<Arc<AtomicBool>> = None;
        let mut watcher: Option<std::thread::JoinHandle<()>> = None;
        loop {
            let pins = gpio_pins(&state);
            if pins != current {
                if let Some(s) = stop.take() {
                    s.store(true, Ordering::Relaxed);
                }
                current = pins.clone();
                if !pins.is_empty() {
                    let (board, _) = super::system::effective_board(&state);
                    let flag = Arc::new(AtomicBool::new(false));
                    stop = Some(flag.clone());
                    // The previous watcher still holds its GPIO lines for up to half a
                    // second (line requests are exclusive): the new one waits for it.
                    watcher = spawn_gpio_thread(
                        board,
                        pins,
                        tx.clone(),
                        flag,
                        state.events.clone(),
                        watcher.take(),
                    );
                }
            }
            tokio::select! {
                r = changes.changed() => if r.is_err() { break },
                Some(gpio) = rx.recv() => {
                    let show = state.store.get();
                    for t in show.settings.triggers.iter().filter(|t| t.kind == TriggerKind::Gpio && t.gpio == Some(gpio)) {
                        match run_action(&state, &t.action).await {
                            Ok(msg) => tracing::info!("Button \"{}\" (GPIO{gpio}): {msg}", t.name),
                            Err(e) => tracing::warn!("Button \"{}\" (GPIO{gpio}) failed: {}", t.name, e.message),
                        }
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "linux")]
fn spawn_gpio_thread(
    board: pixelplus_core::model::BoardKind,
    pins: Vec<u8>,
    tx: tokio::sync::mpsc::UnboundedSender<u8>,
    stop: Arc<AtomicBool>,
    events: crate::events::EventBus,
    previous: Option<std::thread::JoinHandle<()>>,
) -> Option<std::thread::JoinHandle<()>> {
    use pixelplus_hw::gpio::{ButtonSource, GpioButtons, DEFAULT_DEBOUNCE};
    std::thread::Builder::new()
        .name("pp-gpio".into())
        .spawn(move || {
            if let Some(h) = previous {
                let _ = h.join();
            }
            if stop.load(Ordering::Relaxed) {
                return; // replaced again meanwhile
            }
            let mut buttons = match GpioButtons::open(board, &pins, DEFAULT_DEBOUNCE) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Trigger buttons unavailable: {e}");
                    events.toast(
                        crate::events::ToastKind::Warning,
                        format!("Trigger buttons can't be used: {e}"),
                    );
                    return;
                }
            };
            tracing::info!("Watching trigger buttons on GPIO {pins:?}");
            while !stop.load(Ordering::Relaxed) {
                match buttons.wait(std::time::Duration::from_millis(500)) {
                    Ok(Some(ev)) if ev.pressed => {
                        let _ = tx.send(ev.gpio);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Trigger buttons stopped: {e}");
                        return;
                    }
                }
            }
        })
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn spawn_gpio_thread(
    _board: pixelplus_core::model::BoardKind,
    _pins: Vec<u8>,
    _tx: tokio::sync::mpsc::UnboundedSender<u8>,
    _stop: Arc<AtomicBool>,
    _events: crate::events::EventBus,
    _previous: Option<std::thread::JoinHandle<()>>,
) -> Option<std::thread::JoinHandle<()>> {
    tracing::info!("GPIO trigger buttons are only available on a Raspberry Pi");
    None
}
