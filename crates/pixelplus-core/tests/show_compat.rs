//! `show.json` written by PixelPlus 0.1 must keep loading in every later
//! version: a field added without `#[serde(default)]` would make the daemon
//! set the user's show aside and start empty after an update.
//!
//! `testdata/show-v0.1.json` is a real show saved by the daemon (the e2e
//! scenario: three controllers, xLights props, sequences, DJ clip, playlist,
//! schedule), trimmed, plus a receiver, a trigger and integration settings.
//! Never regenerate it; add a new fixture for a new format instead.

use pixelplus_core::model::Show;

const V01: &str = include_str!("../testdata/show-v0.1.json");

#[test]
fn show_json_from_0_1_still_loads() {
    let show: Show = serde_json::from_str(V01).expect("a 0.1 show.json must keep loading");
    assert_eq!(show.name, "E2E Show");
    assert_eq!(show.nodes.len(), 3);
    assert!(!show.props.is_empty());
    assert_eq!(show.sequences.len(), 2);
    assert_eq!(show.receivers.len(), 1);
    assert_eq!(show.settings.triggers.len(), 1);
    assert_eq!(
        show.settings.requests.radio_frequency.as_deref(),
        Some("88.3 FM")
    );
    assert!(show.schedule.enabled);
    // And what this version writes reads back the same.
    let again: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
    assert_eq!(again, show);
}
