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

/// A show saved by the release before the feature wave (HEAD 2026-09: sound
/// delay, units, latch alignment, allowed hosts, channel runs, volume curfew).
const HEAD_2026_09: &str = include_str!("../testdata/show-head-2026-09.json");

/// Keys this version adds to every saved show (always-present settings with
/// defaults). Everything else new is omitted while empty.
const ADDED_KEYS: &[&str] = &[
    "/formatVersion",
    "/settings/https",
    "/settings/reports",
    "/settings/power",
    "/settings/remote",
    "/settings/updates",
    "/settings/xlights",
];

/// Every value of `old` is present, unchanged, in `new`; keys only in `new`
/// are collected in `added`.
fn contained(
    old: &serde_json::Value,
    new: &serde_json::Value,
    path: &str,
    added: &mut Vec<String>,
) {
    use serde_json::Value;
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in a {
                let p = format!("{path}/{k}");
                let w = b
                    .get(k)
                    .unwrap_or_else(|| panic!("{p} was dropped on save"));
                contained(v, w, &p, added);
            }
            for k in b.keys().filter(|k| !a.contains_key(*k)) {
                added.push(format!("{path}/{k}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length changed");
            for (i, (v, w)) in a.iter().zip(b).enumerate() {
                contained(v, w, &format!("{path}/{i}"), added);
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            // f32 fields widen to f64 in a `Value`: compare with f32 precision.
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(
                (a - b).abs() <= 1e-6 * a.abs().max(1.0),
                "{path}: {a} became {b}"
            );
        }
        _ => assert_eq!(old, new, "{path} changed"),
    }
}

fn check_round_trip(src: &str) -> Vec<String> {
    let show: Show = serde_json::from_str(src).expect("show.json must keep loading");
    let saved = serde_json::to_value(&show).unwrap();
    let mut added = vec![];
    contained(&serde_json::from_str(src).unwrap(), &saved, "", &mut added);
    let again: Show = serde_json::from_value(saved).unwrap();
    assert_eq!(again, show);
    added.sort();
    added
}

#[test]
fn show_json_from_head_round_trips_unchanged() {
    let mut want: Vec<String> = ADDED_KEYS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(check_round_trip(HEAD_2026_09), want);
    let show: Show = serde_json::from_str(HEAD_2026_09).unwrap();
    assert_eq!(show.settings.audio.output_delay_ms, 180);
    assert_eq!(show.format_version, 1);
    assert!(show.profiles.is_empty() && show.sensor_nodes.is_empty());
    assert!(show.settings.https.enabled);
}

#[test]
fn show_json_from_0_1_saves_only_known_additions() {
    // 0.1 predates a few HEAD fields too (they are defaults with serde(default)).
    let added = check_round_trip(V01);
    for k in ADDED_KEYS {
        assert!(added.iter().any(|a| a == k), "{k} not added");
    }
}
