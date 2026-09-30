//! Sensor polling: every 5 s read the board's sensors (pixelplus-hw), publish
//! them as the `sensors` WebSocket message, feed alert rules and keep history
//! (last hour at 5 s, last 24 h at 1-minute averages).

use crate::state::AppState;
use parking_lot::Mutex;
use pixelplus_hw::{Sensor, SensorStatus};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

pub const POLL: Duration = Duration::from_secs(5);
const FINE_KEEP_MS: i64 = 60 * 60 * 1000;
const COARSE_STEP_MS: i64 = 60 * 1000;
const COARSE_KEEP_MS: i64 = 24 * 60 * 60 * 1000;

/// A reading as sent to the UI (`Sensor` + which node reports it + status).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    #[serde(flatten)]
    pub sensor: Sensor,
    pub node_id: String,
    pub status: SensorStatus,
}

#[derive(Default)]
struct Series {
    fine: VecDeque<(i64, f64)>,
    coarse: VecDeque<(i64, f64)>,
    /// Accumulator for the current minute: (minute start, sum, count).
    acc: Option<(i64, f64, u32)>,
}

/// Sensor history ring buffers.
#[derive(Default)]
pub struct History {
    series: BTreeMap<String, Series>,
}

impl History {
    pub fn push(&mut self, t_ms: i64, values: &[(String, f64)]) {
        for (id, v) in values {
            let s = self.series.entry(id.clone()).or_default();
            s.fine.push_back((t_ms, *v));
            while s
                .fine
                .front()
                .is_some_and(|(t, _)| *t < t_ms - FINE_KEEP_MS)
            {
                s.fine.pop_front();
            }
            let minute = t_ms - t_ms.rem_euclid(COARSE_STEP_MS);
            match &mut s.acc {
                Some((m, sum, n)) if *m == minute => {
                    *sum += v;
                    *n += 1;
                }
                acc => {
                    if let Some((m, sum, n)) = acc.take() {
                        s.coarse.push_back((m, round3(sum / f64::from(n))));
                    }
                    *acc = Some((minute, *v, 1));
                }
            }
            while s
                .coarse
                .front()
                .is_some_and(|(t, _)| *t < t_ms - COARSE_KEEP_MS)
            {
                s.coarse.pop_front();
            }
        }
    }

    /// Points of the last `minutes` minutes: 5-s points up to an hour, 1-min
    /// averages beyond.
    pub fn query(&self, now_ms: i64, minutes: u32) -> BTreeMap<String, Vec<[f64; 2]>> {
        let minutes = minutes.clamp(1, 24 * 60);
        let from = now_ms - i64::from(minutes) * 60_000;
        self.series
            .iter()
            .map(|(id, s)| {
                let pts: Vec<[f64; 2]> = if minutes <= 60 {
                    s.fine
                        .iter()
                        .filter(|(t, _)| *t >= from)
                        .map(|(t, v)| [*t as f64, *v])
                        .collect()
                } else {
                    let mut v: Vec<[f64; 2]> = s
                        .coarse
                        .iter()
                        .filter(|(t, _)| *t >= from)
                        .map(|(t, v)| [*t as f64, *v])
                        .collect();
                    if let Some((m, sum, n)) = s.acc {
                        v.push([m as f64, round3(sum / f64::from(n))]);
                    }
                    v
                };
                (id.clone(), pts)
            })
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Default)]
pub struct SensorState {
    latest: Mutex<Vec<Reading>>,
    history: Mutex<History>,
}

impl SensorState {
    pub fn latest(&self) -> Vec<Reading> {
        self.latest.lock().clone()
    }

    pub fn history(&self, minutes: u32) -> BTreeMap<String, Vec<[f64; 2]>> {
        self.history
            .lock()
            .query(chrono::Utc::now().timestamp_millis(), minutes)
    }

    /// Record a new set of readings (also used by tests).
    pub fn record(&self, readings: Vec<Reading>) {
        let values: Vec<(String, f64)> = readings
            .iter()
            .map(|r| (r.sensor.id.clone(), r.sensor.value))
            .collect();
        self.history
            .lock()
            .push(chrono::Utc::now().timestamp_millis(), &values);
        *self.latest.lock() = readings;
    }
}

/// Start the polling task.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let (board, _) = tokio::task::spawn_blocking({
            let s = state.clone();
            move || super::system::effective_board(&s)
        })
        .await
        .unwrap_or((pixelplus_core::model::BoardKind::Virtual, None));
        let hub = std::sync::Arc::new(Mutex::new(make_hub(board)));
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            // Re-create the hub if the board was changed in the wizard.
            let (now_board, _) = super::system::effective_board(&state);
            if now_board != hub.lock().board() {
                *hub.lock() = make_hub(now_board);
            }
            let h = hub.clone();
            let Ok(sensors) = tokio::task::spawn_blocking(move || h.lock().read_all()).await else {
                continue;
            };
            let node_id = state.identity().id;
            let readings: Vec<Reading> = sensors
                .into_iter()
                .map(|s| Reading {
                    status: s.status(),
                    sensor: s,
                    node_id: node_id.clone(),
                })
                .collect();
            state.services.sensors.record(readings.clone());
            if let Ok(v) = serde_json::to_value(&readings) {
                state.services.remember("sensors", v.clone());
                state.events.publish("sensors", &v);
            }
            super::alerts::check_sensors(&state, &readings).await;
        }
    });
}

fn make_hub(board: pixelplus_core::model::BoardKind) -> pixelplus_hw::SensorHub {
    #[cfg(target_os = "linux")]
    {
        let bus = pixelplus_hw::LinuxI2c::open(pixelplus_hw::i2c::DEFAULT_BUS)
            .ok()
            .map(|b| Box::new(b) as Box<dyn pixelplus_hw::I2cBus>);
        pixelplus_hw::SensorHub::new(board, bus)
    }
    #[cfg(not(target_os = "linux"))]
    {
        pixelplus_hw::SensorHub::new(board, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_fine_and_coarse() {
        let mut h = History::default();
        let t0 = 1_700_000_000_000i64 - 1_700_000_000_000i64.rem_euclid(60_000);
        // Two hours of 5-s samples.
        for i in 0..(2 * 720) {
            let t = t0 + i * 5000;
            h.push(t, &[("cpuTemp".into(), 40.0 + (i % 12) as f64)]);
        }
        let now = t0 + 2 * 3_600_000;
        let fine = h.query(now, 10);
        let pts = &fine["cpuTemp"];
        assert!((119..=121).contains(&pts.len()), "{}", pts.len());
        let coarse = h.query(now, 120);
        let c = &coarse["cpuTemp"];
        assert!((119..=121).contains(&c.len()), "{}", c.len());
        // Each full minute averages 40..51 -> 45.5.
        assert_eq!(c[0][1], 45.5);
        // Fine buffer only keeps an hour.
        assert!(h.query(now, 60)["cpuTemp"].len() <= 721);
    }
}
