//! Feature toggles (Settings → Features, ARCHITECTURE §12.17).
//!
//! Every optional part of PixelPlus has a [`FeatureId`]. A show keeps the
//! features that are **turned off** in `settings.features.disabled`, so a show
//! written before this existed (no `features` key) has everything on — nobody
//! who upgrades loses anything. New shows made by the setup wizard start from
//! a preset ([`FeatureSettings::essentials`] by default).
//!
//! Turning a feature off never deletes its content (clips, looks, seasons,
//! ROMs…): the interface hides it, its API answers `feature_disabled`, its
//! background work stops and playlist items of that kind are skipped. Turning
//! it back on restores everything as it was.
//!
//! Ids are stored as strings so a show written by a newer PixelPlus (with
//! features this one does not know) still loads and keeps them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One optional feature. The camelCase name is its id in JSON and in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureId {
    // --- Show extras
    Dj,
    Effects,
    AutoShows,
    SmartPlaylists,
    Countdown,
    Seasons,
    Requests,
    Games,
    // --- Setup tools
    Layout,
    FaultFinder,
    PixelCount,
    ReceiverWizard,
    MapYard,
    SoundSync,
    PhoneTrust,
    // --- Running the show
    Reports,
    Alerts,
    Power,
    Triggers,
    Sensors,
    Surprises,
    Mqtt,
    Remote,
    XlightsUpload,
}

/// Catalogue group (the Features page shows one card per group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureGroup {
    Show,
    Setup,
    Operations,
}

impl FeatureId {
    /// Every feature, in catalogue order.
    pub const ALL: [FeatureId; 24] = [
        FeatureId::Dj,
        FeatureId::Effects,
        FeatureId::AutoShows,
        FeatureId::SmartPlaylists,
        FeatureId::Countdown,
        FeatureId::Seasons,
        FeatureId::Requests,
        FeatureId::Games,
        FeatureId::Layout,
        FeatureId::FaultFinder,
        FeatureId::PixelCount,
        FeatureId::ReceiverWizard,
        FeatureId::MapYard,
        FeatureId::SoundSync,
        FeatureId::PhoneTrust,
        FeatureId::Reports,
        FeatureId::Alerts,
        FeatureId::Power,
        FeatureId::Triggers,
        FeatureId::Sensors,
        FeatureId::Surprises,
        FeatureId::Mqtt,
        FeatureId::Remote,
        FeatureId::XlightsUpload,
    ];

    /// The JSON id (`"mapYard"`).
    pub fn as_str(self) -> &'static str {
        match self {
            FeatureId::Dj => "dj",
            FeatureId::Effects => "effects",
            FeatureId::AutoShows => "autoShows",
            FeatureId::SmartPlaylists => "smartPlaylists",
            FeatureId::Countdown => "countdown",
            FeatureId::Seasons => "seasons",
            FeatureId::Requests => "requests",
            FeatureId::Games => "games",
            FeatureId::Layout => "layout",
            FeatureId::FaultFinder => "faultFinder",
            FeatureId::PixelCount => "pixelCount",
            FeatureId::ReceiverWizard => "receiverWizard",
            FeatureId::MapYard => "mapYard",
            FeatureId::SoundSync => "soundSync",
            FeatureId::PhoneTrust => "phoneTrust",
            FeatureId::Reports => "reports",
            FeatureId::Alerts => "alerts",
            FeatureId::Power => "power",
            FeatureId::Triggers => "triggers",
            FeatureId::Sensors => "sensors",
            FeatureId::Surprises => "surprises",
            FeatureId::Mqtt => "mqtt",
            FeatureId::Remote => "remote",
            FeatureId::XlightsUpload => "xlightsUpload",
        }
    }

    pub fn parse(id: &str) -> Option<FeatureId> {
        FeatureId::ALL.into_iter().find(|f| f.as_str() == id)
    }

    /// Plain-language name, as on the Features page.
    pub fn name(self) -> &'static str {
        match self {
            FeatureId::Dj => "DJ Studio",
            FeatureId::Effects => "Effects & looks",
            FeatureId::AutoShows => "Light shows from music",
            FeatureId::SmartPlaylists => "Smart playlists & tags",
            FeatureId::Countdown => "Countdown to showtime",
            FeatureId::Seasons => "Seasons",
            FeatureId::Requests => "Song requests",
            FeatureId::Games => "Games",
            FeatureId::Layout => "Layout & preview",
            FeatureId::FaultFinder => "Fault finder",
            FeatureId::PixelCount => "Pixel count check",
            FeatureId::ReceiverWizard => "Receiver wizard",
            FeatureId::MapYard => "Map my yard",
            FeatureId::SoundSync => "Sync to sound",
            FeatureId::PhoneTrust => "Phone trust (HTTPS)",
            FeatureId::Reports => "Nightly report",
            FeatureId::Alerts => "Alerts",
            FeatureId::Power => "Power limiter",
            FeatureId::Triggers => "Buttons & triggers",
            FeatureId::Sensors => "Sensor nodes",
            FeatureId::Surprises => "Surprises",
            FeatureId::Mqtt => "Home Assistant & MQTT",
            FeatureId::Remote => "Remote access",
            FeatureId::XlightsUpload => "Upload from xLights",
        }
    }

    pub fn group(self) -> FeatureGroup {
        use FeatureId::*;
        match self {
            Dj | Effects | AutoShows | SmartPlaylists | Countdown | Seasons | Requests | Games => {
                FeatureGroup::Show
            }
            Layout | FaultFinder | PixelCount | ReceiverWizard | MapYard | SoundSync
            | PhoneTrust => FeatureGroup::Setup,
            Reports | Alerts | Power | Triggers | Sensors | Surprises | Mqtt | Remote
            | XlightsUpload => FeatureGroup::Operations,
        }
    }

    /// Features this one needs; turning one of them off turns this off too.
    pub fn requires(self) -> &'static [FeatureId] {
        match self {
            // The phone camera and microphone only work on a secure page.
            FeatureId::MapYard | FeatureId::SoundSync => &[FeatureId::PhoneTrust],
            // Sensor inputs and surprises are delivered through triggers.
            FeatureId::Sensors | FeatureId::Surprises => &[FeatureId::Triggers],
            _ => &[],
        }
    }

    /// Features that need this one (the reverse of [`FeatureId::requires`]).
    pub fn dependents(self) -> Vec<FeatureId> {
        FeatureId::ALL
            .into_iter()
            .filter(|f| f.requires().contains(&self))
            .collect()
    }

    /// The friendly `feature_disabled` message.
    pub fn off_message(self) -> String {
        format!(
            "{} is turned off on this controller. Turn it on in Settings → Features.",
            self.name()
        )
    }
}

/// `settings.features`: which optional features are turned off.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FeatureSettings {
    /// Ids of the features that are off (sorted, no duplicates). Unknown ids
    /// (from a newer PixelPlus) are kept as they are.
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// The features of the "Essentials" preset: the everyday setup tools a new
/// display needs, plus the power limiter (it protects fuses and supplies).
pub const ESSENTIALS: [FeatureId; 6] = [
    FeatureId::Effects,
    FeatureId::Layout,
    FeatureId::FaultFinder,
    FeatureId::PixelCount,
    FeatureId::ReceiverWizard,
    FeatureId::Power,
];

impl FeatureSettings {
    /// Everything on (what every show had before feature toggles).
    pub fn everything() -> Self {
        FeatureSettings::default()
    }

    /// The "Essentials" preset ([`ESSENTIALS`] on, the rest off).
    pub fn essentials() -> Self {
        Self::only(&ESSENTIALS)
    }

    /// Exactly `on` enabled (plus whatever those need).
    pub fn only(on: &[FeatureId]) -> Self {
        let mut s = FeatureSettings {
            disabled: FeatureId::ALL
                .into_iter()
                .filter(|f| !on.contains(f))
                .map(|f| f.as_str().to_string())
                .collect(),
        };
        for f in on {
            s.set(*f, true);
        }
        s
    }

    pub fn is_enabled(&self, id: FeatureId) -> bool {
        !self.disabled.iter().any(|d| d == id.as_str())
    }

    /// The known features that are off.
    pub fn disabled_ids(&self) -> BTreeSet<FeatureId> {
        self.disabled
            .iter()
            .filter_map(|d| FeatureId::parse(d))
            .collect()
    }

    /// Turn one feature on or off, keeping dependencies consistent: turning a
    /// feature on also turns on what it needs; turning one off also turns off
    /// what needs it. Returns every feature whose state changed (including
    /// `id`), in catalogue order.
    pub fn set(&mut self, id: FeatureId, on: bool) -> Vec<FeatureId> {
        let before = self.disabled_ids();
        let mut off: BTreeSet<String> = self.disabled.iter().cloned().collect();
        if on {
            let mut todo = vec![id];
            while let Some(f) = todo.pop() {
                if off.remove(f.as_str()) || f == id {
                    todo.extend_from_slice(f.requires());
                }
            }
        } else {
            let mut todo = vec![id];
            while let Some(f) = todo.pop() {
                if off.insert(f.as_str().to_string()) || f == id {
                    todo.extend(f.dependents());
                }
            }
        }
        self.disabled = off.into_iter().collect();
        self.normalize();
        let after = self.disabled_ids();
        FeatureId::ALL
            .into_iter()
            .filter(|f| before.contains(f) != after.contains(f))
            .collect()
    }

    /// Sort and dedupe, and turn off every feature whose requirement is off
    /// (so a hand-edited or partial list is always consistent).
    pub fn normalize(&mut self) {
        let mut off: BTreeSet<String> = self
            .disabled
            .iter()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        loop {
            let extra: Vec<FeatureId> = FeatureId::ALL
                .into_iter()
                .filter(|f| !off.contains(f.as_str()))
                .filter(|f| f.requires().iter().any(|r| off.contains(r.as_str())))
                .collect();
            if extra.is_empty() {
                break;
            }
            off.extend(extra.into_iter().map(|f| f.as_str().to_string()));
        }
        self.disabled = off.into_iter().collect();
    }
}

/// The feature a playlist item belongs to, if it is an optional kind
/// (DJ clips, countdowns, looks, game commands).
pub fn playlist_item_feature(item: &crate::model::PlaylistItem) -> Option<FeatureId> {
    use crate::model::PlaylistItem as I;
    match item {
        I::Dj { .. } => Some(FeatureId::Dj),
        I::Countdown { .. } => Some(FeatureId::Countdown),
        I::Effect { .. } => Some(FeatureId::Effects),
        I::Command { command, .. } if command.starts_with("games.") => Some(FeatureId::Games),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip_and_are_unique() {
        let mut seen = BTreeSet::new();
        for f in FeatureId::ALL {
            assert_eq!(FeatureId::parse(f.as_str()), Some(f));
            assert!(seen.insert(f.as_str()));
            let json = serde_json::to_value(f).unwrap();
            assert_eq!(json, serde_json::json!(f.as_str()));
            assert!(!f.name().is_empty());
        }
        assert_eq!(FeatureId::parse("nope"), None);
    }

    #[test]
    fn default_is_everything_on() {
        let s = FeatureSettings::default();
        assert!(FeatureId::ALL.iter().all(|f| s.is_enabled(*f)));
        let parsed: FeatureSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn essentials_preset() {
        let s = FeatureSettings::essentials();
        for f in FeatureId::ALL {
            assert_eq!(s.is_enabled(f), ESSENTIALS.contains(&f), "{f:?}");
        }
    }

    #[test]
    fn turning_on_turns_on_what_it_needs() {
        let mut s = FeatureSettings::essentials();
        let changed = s.set(FeatureId::MapYard, true);
        assert_eq!(changed, vec![FeatureId::MapYard, FeatureId::PhoneTrust]);
        assert!(s.is_enabled(FeatureId::PhoneTrust));
        assert!(!s.is_enabled(FeatureId::SoundSync));
    }

    #[test]
    fn turning_off_turns_off_what_needs_it() {
        let mut s = FeatureSettings::everything();
        let changed = s.set(FeatureId::Triggers, false);
        assert_eq!(
            changed,
            vec![
                FeatureId::Triggers,
                FeatureId::Sensors,
                FeatureId::Surprises
            ]
        );
        // Turning a dependent back on brings its requirement back.
        let changed = s.set(FeatureId::Surprises, true);
        assert_eq!(changed, vec![FeatureId::Triggers, FeatureId::Surprises]);
        assert!(!s.is_enabled(FeatureId::Sensors));
    }

    #[test]
    fn normalize_is_consistent_and_keeps_unknown_ids() {
        let mut s = FeatureSettings {
            disabled: vec![
                "phoneTrust".into(),
                "fromTheFuture".into(),
                "games".into(),
                "games".into(),
            ],
        };
        s.normalize();
        assert_eq!(
            s.disabled,
            [
                "fromTheFuture",
                "games",
                "mapYard",
                "phoneTrust",
                "soundSync"
            ]
        );
        assert!(!s.is_enabled(FeatureId::MapYard));
        assert!(s.is_enabled(FeatureId::Dj));
    }

    #[test]
    fn playlist_items_map_to_features() {
        use crate::model::PlaylistItem as I;
        let dj = I::Dj {
            id: "a".into(),
            dj_clip_id: "c".into(),
        };
        assert_eq!(playlist_item_feature(&dj), Some(FeatureId::Dj));
        let seq = I::Sequence {
            id: "a".into(),
            sequence_id: "s".into(),
        };
        assert_eq!(playlist_item_feature(&seq), None);
        let cmd = I::Command {
            id: "a".into(),
            command: "games.invite".into(),
            args: Default::default(),
        };
        assert_eq!(playlist_item_feature(&cmd), Some(FeatureId::Games));
    }
}
