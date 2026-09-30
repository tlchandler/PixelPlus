//! OLED status screen (SSD1306 on the difftxlarge J22 socket): node name,
//! player state, current song, IP address and temperature, refreshed every
//! 2 s while `settings.oled.enabled`.

use crate::state::AppState;
use pixelplus_hw::oled::StatusScreen;
use std::time::Duration;

/// What the screen should show now.
pub fn screen(state: &AppState) -> StatusScreen {
    let id = state.identity();
    let show = state.store.get();
    let name = id.name.clone().unwrap_or_else(|| show.name.clone());
    let (st, song) = match state.services.player.get().map(|p| p.status()) {
        Some(s) => {
            let label = match s.state {
                crate::player::PlayerState::Idle if s.blackout => "Blackout",
                crate::player::PlayerState::Idle => "Idle",
                crate::player::PlayerState::Playing => "Playing",
                crate::player::PlayerState::Paused => "Paused",
                crate::player::PlayerState::Testing => "Testing",
                crate::player::PlayerState::Effect => "Look",
            };
            (label.to_string(), s.item.map(|i| i.name))
        }
        None => ("Starting".to_string(), None),
    };
    let temp_c = state
        .services
        .sensors
        .latest()
        .iter()
        .filter(|r| r.sensor.kind == pixelplus_hw::SensorKind::Temperature)
        .map(|r| r.sensor.value)
        .fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))))
        .or_else(|| super::system::soc_temp().map(f64::from));
    // While a phone compares the secure-connection fingerprint (F1: /trust
    // page or Settings → Secure connection), show it instead of the song.
    let song = super::tls::oled_line(state).or(song);
    StatusScreen {
        name,
        state: st,
        song,
        ip: super::system::ip_addresses().into_iter().next(),
        temp_c,
    }
}

pub fn start(state: &AppState) {
    #[cfg(target_os = "linux")]
    {
        let state = state.clone();
        tokio::spawn(async move {
            use parking_lot::Mutex;
            use pixelplus_hw::oled::{Canvas, Ssd1306, OLED_ADDR};
            use std::sync::Arc;
            type Panel = Ssd1306<pixelplus_hw::LinuxI2c>;
            let panel: Arc<Mutex<Option<Panel>>> = Arc::new(Mutex::new(None));
            let mut was_on = false;
            let mut failed = false;
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tick.tick().await;
                let enabled = state.store.get().settings.oled.enabled;
                if !enabled {
                    if was_on {
                        let p = panel.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Some(d) = p.lock().as_mut() {
                                let _ = d.set_power(false);
                            }
                        })
                        .await;
                        was_on = false;
                    }
                    failed = false;
                    continue;
                }
                if failed {
                    continue;
                }
                let screen = screen(&state);
                let p = panel.clone();
                let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
                    let mut guard = p.lock();
                    if guard.is_none() {
                        let bus = pixelplus_hw::LinuxI2c::open(pixelplus_hw::i2c::DEFAULT_BUS)
                            .map_err(|e| e.to_string())?;
                        let mut d = Ssd1306::new(bus, OLED_ADDR);
                        d.init().map_err(|e| e.to_string())?;
                        *guard = Some(d);
                    }
                    let d = guard.as_mut().expect("panel");
                    let mut canvas = Canvas::new();
                    screen.render(&mut canvas);
                    d.set_power(true).map_err(|e| e.to_string())?;
                    d.flush(&canvas).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                match res {
                    Ok(()) => was_on = true,
                    Err(e) => {
                        tracing::warn!("The OLED status screen isn't responding ({e}); turn it off in Settings if none is fitted");
                        *panel.lock() = None;
                        failed = true;
                    }
                }
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = state;
}
