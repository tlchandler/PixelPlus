//! Reacting to feature toggles (Settings → Features, ARCHITECTURE §12.17).
//!
//! Most services read `show.feature(..)` wherever they decide to do work, so
//! a change applies on their next look at the show (MQTT disconnects, the
//! sensor-node port closes, the nightly report and analysis jobs pause, the
//! power limiter and HTTPS listener stop, the engine skips playlist items).
//! This task handles what needs a nudge: the games sidecar is told to reload
//! its settings (it stops its phone page and invites when games are off),
//! and every change is logged and announced to open browsers.

use crate::state::AppState;
use pixelplus_core::features::FeatureId;
use serde_json::json;
use std::collections::BTreeSet;

/// Features that went off / on between two disabled sets.
pub fn diff(
    before: &BTreeSet<FeatureId>,
    after: &BTreeSet<FeatureId>,
) -> (Vec<FeatureId>, Vec<FeatureId>) {
    let off = after.difference(before).copied().collect();
    let on = before.difference(after).copied().collect();
    (off, on)
}

pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut changes = state.store.subscribe();
        let mut last = state.store.get().settings.features.disabled_ids();
        while changes.changed().await.is_ok() {
            let now = state.store.get().settings.features.disabled_ids();
            if now == last {
                continue;
            }
            let (off, on) = diff(&last, &now);
            last = now;
            for f in &off {
                tracing::info!("Feature turned off: {}", f.name());
            }
            for f in &on {
                tracing::info!("Feature turned on: {}", f.name());
            }
            state
                .events
                .publish("features", &json!({ "turnedOff": off, "turnedOn": on }));
            if off.contains(&FeatureId::Games) || on.contains(&FeatureId::Games) {
                let st = state.clone();
                tokio::spawn(async move {
                    if !st.store.get().feature(FeatureId::Games) {
                        // End a running game at once; reload closes the phone page.
                        let _ = super::games::command(&st, json!({"cmd": "stop"})).await;
                    }
                    let _ = super::games::command(&st, json!({"cmd": "reload"})).await;
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_reports_both_directions() {
        let a: BTreeSet<_> = [FeatureId::Games, FeatureId::Dj].into();
        let b: BTreeSet<_> = [FeatureId::Dj, FeatureId::Mqtt].into();
        assert_eq!(
            diff(&a, &b),
            (vec![FeatureId::Mqtt], vec![FeatureId::Games])
        );
    }
}
