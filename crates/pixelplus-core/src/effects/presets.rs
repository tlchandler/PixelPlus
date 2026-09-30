//! Built-in looks offered on the Effects page and seeded into new shows.

use crate::model::{EffectKind, EffectParams, EffectPreset, Target};
use serde_json::{json, Value};

fn preset(id: &str, name: &str, effect: EffectKind, params: &[(&str, Value)]) -> EffectPreset {
    EffectPreset {
        id: format!("builtin-{id}"),
        name: name.into(),
        effect,
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<EffectParams>(),
        target: Target {
            all: true,
            ..Target::default()
        },
    }
}

/// Curated, named looks. Ids are stable (`builtin-…`) so shows can refer to
/// them; every preset targets all props.
pub fn builtin_presets() -> Vec<EffectPreset> {
    use EffectKind as K;
    vec![
        preset(
            "warm-white-twinkle",
            "Warm White Twinkle",
            K::Twinkle,
            &[
                ("colors", json!(["#ffb46b"])),
                ("density", json!(0.5)),
                ("speed", json!(0.6)),
                ("glow", json!(0.25)),
            ],
        ),
        preset(
            "candy-cane-chase",
            "Candy Cane Chase",
            K::Candycane,
            &[
                ("colors", json!(["#ff0000", "#ffffff"])),
                ("stripeWidth", json!(4.0)),
                ("speed", json!(4.0)),
                ("brightness", json!(80.0)),
            ],
        ),
        preset(
            "classic-christmas",
            "Classic Christmas Colors",
            K::Chase,
            &[
                (
                    "colors",
                    json!(["#ff0000", "#00b000", "#0030ff", "#ff6a00", "#ff00a0"]),
                ),
                ("size", json!(1.0)),
                ("gap", json!(0.0)),
                ("speed", json!(0.0)),
                ("fade", json!(false)),
            ],
        ),
        preset(
            "winter-snowfall",
            "Winter Snowfall",
            K::Snow,
            &[
                ("color", json!("#ffffff")),
                ("background", json!("#00061a")),
                ("density", json!(0.4)),
                ("speed", json!(0.2)),
                ("wind", json!(0.15)),
            ],
        ),
        preset(
            "rainbow-wave",
            "Rainbow Wave",
            K::Rainbow,
            &[
                ("mode", json!("across")),
                ("speed", json!(0.2)),
                ("spread", json!(1.0)),
            ],
        ),
        preset(
            "fireplace-glow",
            "Fireplace Glow",
            K::Fire,
            &[
                ("palette", json!("classic")),
                ("height", json!(0.7)),
                ("speed", json!(0.8)),
            ],
        ),
        preset(
            "north-pole-blue",
            "North Pole Blue",
            K::Wave,
            &[
                ("colors", json!(["#001a66", "#0066ff", "#aee4ff"])),
                ("speed", json!(0.15)),
                ("wavelength", json!(0.8)),
                ("direction", json!("right")),
            ],
        ),
        preset(
            "starry-night",
            "Starry Night",
            K::Sparkle,
            &[
                ("colors", json!(["#020818"])),
                ("sparkleColor", json!("#fff4e0")),
                ("density", json!(0.05)),
                ("speed", json!(0.6)),
            ],
        ),
        preset(
            "icicle-drip",
            "Icicle Drip",
            K::Meteor,
            &[
                ("colors", json!(["#cfe8ff"])),
                ("speed", json!(25.0)),
                ("tailLength", json!(12.0)),
                ("count", json!(3.0)),
            ],
        ),
        preset(
            "red-green-wash",
            "Red & Green Wash",
            K::Colorwash,
            &[
                ("colors", json!(["#ff0000", "#00c000"])),
                ("speed", json!(0.05)),
                ("spread", json!(0.5)),
            ],
        ),
        preset(
            "candlelight-breathe",
            "Candlelight Breathe",
            K::Breathe,
            &[
                ("colors", json!(["#ff9a3c"])),
                ("period", json!(5.0)),
                ("minBrightness", json!(0.2)),
            ],
        ),
        preset(
            "stars-and-stripes",
            "Stars & Stripes",
            K::Chase,
            &[
                ("colors", json!(["#ff0000", "#ffffff", "#0033ff"])),
                ("size", json!(4.0)),
                ("gap", json!(0.0)),
                ("speed", json!(6.0)),
                ("fade", json!(false)),
            ],
        ),
        preset(
            "spooky-flames",
            "Spooky Purple Flames",
            K::Fire,
            &[("palette", json!("purple")), ("height", json!(0.9))],
        ),
        preset(
            "silver-sparkle-strobe",
            "Silver Sparkle Strobe",
            K::Strobe,
            &[
                ("color", json!("#e0e8ff")),
                ("rate", json!(6.0)),
                ("duty", json!(0.1)),
                ("pattern", json!("random")),
                ("brightness", json!(70.0)),
            ],
        ),
        preset(
            "solid-warm-white",
            "Solid Warm White",
            K::Solid,
            &[("color", json!("#ffb46b")), ("brightness", json!(70.0))],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{param_schema, resolve_params};

    #[test]
    fn presets_are_valid_and_unique() {
        let presets = builtin_presets();
        assert!(presets.len() >= 12);
        let mut ids: Vec<_> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), presets.len());
        for p in &presets {
            let keys: Vec<_> = param_schema(p.effect).into_iter().map(|s| s.key).collect();
            let resolved = resolve_params(p.effect, &p.params);
            for (k, v) in &p.params {
                assert!(keys.contains(k), "{}: unknown param {k}", p.name);
                assert_eq!(&resolved[k], v, "{}: {k} changed by validation", p.name);
            }
            assert!(p.target.all);
        }
    }
}
