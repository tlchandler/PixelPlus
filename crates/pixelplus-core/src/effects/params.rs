//! Effect parameter schemas and validation.
//!
//! [`param_schema`] describes every parameter an effect understands so the UI
//! can build editors automatically. [`resolve_params`] turns whatever a user
//! (or an old show file) stored into a complete, valid parameter set: missing
//! keys get defaults, numbers are clamped, bad colours and unknown options fall
//! back to defaults, and unknown keys are dropped. The renderer only ever sees
//! resolved parameters.

use super::color::Rgb;
use crate::model::{EffectKind, EffectParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Editor widget for a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParamKind {
    /// One `"#rrggbb"` colour.
    Color,
    /// A list of `"#rrggbb"` colours (at least one).
    Colors,
    /// A number between `min` and `max`.
    Number,
    /// On/off.
    Bool,
    /// One of `options`.
    Select,
}

/// Description of one effect parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    /// Key in `EffectPreset.params`.
    pub key: String,
    /// Short label for the editor.
    pub label: String,
    pub kind: ParamKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Default value (JSON of the parameter's type).
    pub default: Value,
    /// Allowed values for [`ParamKind::Select`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Unit shown after numbers ("px/s", "Hz", "%", "s").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// One-sentence explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl ParamSpec {
    fn new(key: &str, label: &str, kind: ParamKind, default: Value) -> Self {
        ParamSpec {
            key: key.into(),
            label: label.into(),
            kind,
            min: None,
            max: None,
            step: None,
            default,
            options: Vec::new(),
            unit: None,
            help: None,
        }
    }

    fn color(key: &str, label: &str, default: &str) -> Self {
        Self::new(key, label, ParamKind::Color, json!(default))
    }

    fn colors(key: &str, label: &str, default: &[&str]) -> Self {
        Self::new(key, label, ParamKind::Colors, json!(default))
    }

    fn number(key: &str, label: &str, min: f64, max: f64, step: f64, default: f64) -> Self {
        let mut s = Self::new(key, label, ParamKind::Number, json!(default));
        s.min = Some(min);
        s.max = Some(max);
        s.step = Some(step);
        s
    }

    fn boolean(key: &str, label: &str, default: bool) -> Self {
        Self::new(key, label, ParamKind::Bool, json!(default))
    }

    fn select(key: &str, label: &str, options: &[&str], default: &str) -> Self {
        let mut s = Self::new(key, label, ParamKind::Select, json!(default));
        s.options = options.iter().map(|o| o.to_string()).collect();
        s
    }

    fn unit(mut self, unit: &str) -> Self {
        self.unit = Some(unit.into());
        self
    }

    fn help(mut self, help: &str) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Coerce `value` into a valid value for this parameter, or `None` if it
    /// cannot be interpreted.
    fn coerce(&self, value: &Value) -> Option<Value> {
        match self.kind {
            ParamKind::Number => {
                let n = match value {
                    Value::Number(n) => n.as_f64(),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                    _ => None,
                }
                .filter(|n| n.is_finite())?;
                let n = n.clamp(
                    self.min.unwrap_or(f64::MIN),
                    self.max.unwrap_or(f64::MAX),
                );
                Some(json!(n))
            }
            ParamKind::Bool => match value {
                Value::Bool(b) => Some(json!(*b)),
                Value::Number(n) => n.as_f64().map(|n| json!(n != 0.0)),
                Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => Some(json!(true)),
                    "false" | "no" | "off" | "0" => Some(json!(false)),
                    _ => None,
                },
                _ => None,
            },
            ParamKind::Color => match value {
                Value::String(s) => Rgb::from_hex(s).map(|c| json!(c.to_hex())),
                Value::Array(a) => a
                    .iter()
                    .find_map(|v| v.as_str().and_then(Rgb::from_hex))
                    .map(|c| json!(c.to_hex())),
                _ => None,
            },
            ParamKind::Colors => {
                let list: Vec<String> = match value {
                    Value::String(s) => Rgb::from_hex(s).map(|c| c.to_hex()).into_iter().collect(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(|v| v.as_str().and_then(Rgb::from_hex))
                        .map(|c| c.to_hex())
                        .take(MAX_COLORS)
                        .collect(),
                    _ => Vec::new(),
                };
                (!list.is_empty()).then(|| json!(list))
            }
            ParamKind::Select => value
                .as_str()
                .map(str::trim)
                .filter(|s| self.options.iter().any(|o| o == s))
                .map(|s| json!(s)),
        }
    }
}

/// Most colours kept in a `colors` list.
pub const MAX_COLORS: usize = 16;

fn brightness() -> ParamSpec {
    ParamSpec::number("brightness", "Brightness", 0.0, 100.0, 1.0, 100.0)
        .unit("%")
        .help("Overall brightness of this look.")
}

fn direction() -> ParamSpec {
    ParamSpec::select("direction", "Direction", &["forward", "reverse"], "forward")
        .help("Which way along the prop's pixels the pattern moves.")
}

/// Parameters understood by an effect, in display order.
pub fn param_schema(kind: EffectKind) -> Vec<ParamSpec> {
    use ParamSpec as P;
    let mut v = match kind {
        EffectKind::Solid => vec![P::color("color", "Color", "#ffb46b")],
        EffectKind::Chase => vec![
            P::colors("colors", "Colors", &["#ff0000", "#00c000"])
                .help("Each band of lit pixels takes the next colour."),
            P::color("background", "Background", "#000000"),
            P::number("speed", "Speed", 0.0, 60.0, 0.5, 8.0)
                .unit("px/s")
                .help("0 holds the pattern still."),
            P::number("size", "Band size", 1.0, 50.0, 1.0, 3.0).unit("px"),
            P::number("gap", "Gap", 0.0, 50.0, 1.0, 3.0)
                .unit("px")
                .help("Unlit pixels between bands."),
            direction(),
            P::boolean("fade", "Fading tail", true),
        ],
        EffectKind::Twinkle => vec![
            P::colors("colors", "Colors", &["#ffb46b"]),
            P::number("density", "Density", 0.0, 1.0, 0.01, 0.4)
                .help("Share of pixels twinkling at any moment."),
            P::number("speed", "Speed", 0.1, 5.0, 0.1, 1.0),
            P::number("glow", "Base glow", 0.0, 0.8, 0.01, 0.1)
                .help("How bright pixels stay between twinkles."),
        ],
        EffectKind::Rainbow => vec![
            P::number("speed", "Speed", 0.0, 5.0, 0.05, 0.25)
                .unit("cycles/s"),
            P::number("spread", "Rainbows across", 0.1, 10.0, 0.1, 1.0),
            P::number("saturation", "Saturation", 0.0, 1.0, 0.01, 1.0),
            P::select("mode", "Spread", &["along", "across"], "along")
                .help("Along each prop's pixels, or across the whole display."),
            direction(),
        ],
        EffectKind::Colorwash => vec![
            P::colors("colors", "Colors", &["#ff0000", "#00c000", "#0040ff"]),
            P::number("speed", "Speed", 0.0, 2.0, 0.01, 0.05)
                .unit("cycles/s")
                .help("Trips through the whole colour list per second."),
            P::number("spread", "Spread", 0.0, 2.0, 0.05, 0.0)
                .help("0 = every prop the same colour; higher staggers colours across the display."),
        ],
        EffectKind::Candycane => vec![
            P::colors("colors", "Stripe colors", &["#ff0000", "#ffffff"]),
            P::number("stripeWidth", "Stripe width", 1.0, 50.0, 1.0, 4.0).unit("px"),
            P::number("speed", "Speed", 0.0, 30.0, 0.5, 3.0).unit("px/s"),
            direction(),
        ],
        EffectKind::Fire => vec![
            P::select(
                "palette",
                "Flame color",
                &["classic", "ember", "blue", "green", "purple"],
                "classic",
            ),
            P::number("height", "Flame height", 0.1, 1.5, 0.05, 0.8),
            P::number("speed", "Speed", 0.1, 4.0, 0.1, 1.0),
        ],
        EffectKind::Snow => vec![
            P::color("color", "Snow color", "#ffffff"),
            P::color("background", "Sky color", "#00061a"),
            P::number("density", "Amount of snow", 0.0, 1.0, 0.01, 0.35),
            P::number("speed", "Fall speed", 0.05, 2.0, 0.05, 0.25)
                .help("Prop heights per second."),
            P::number("flakeSize", "Flake size", 0.01, 0.3, 0.01, 0.06),
            P::number("wind", "Wind", -1.0, 1.0, 0.05, 0.0),
        ],
        EffectKind::Sparkle => vec![
            P::colors("colors", "Background colors", &["#0a1a4a"]),
            P::color("sparkleColor", "Sparkle color", "#ffffff"),
            P::number("density", "Density", 0.0, 1.0, 0.01, 0.08),
            P::number("speed", "Speed", 0.2, 5.0, 0.1, 1.0),
        ],
        EffectKind::Wave => vec![
            P::colors("colors", "Colors", &["#0020ff", "#00c8ff", "#ffffff"]),
            P::number("speed", "Speed", 0.0, 5.0, 0.05, 0.3).unit("waves/s"),
            P::number("wavelength", "Wave length", 0.05, 4.0, 0.05, 0.5)
                .help("Length of one wave as a share of the display (or prop)."),
            P::select(
                "direction",
                "Direction",
                &["right", "left", "up", "down", "out", "in"],
                "right",
            ),
            P::select("mode", "Spread", &["across", "along"], "across")
                .help("Across the whole display, or along each prop's pixels."),
        ],
        EffectKind::Meteor => vec![
            P::colors("colors", "Colors", &["#ffffff"]),
            P::number("speed", "Speed", 1.0, 200.0, 1.0, 30.0).unit("px/s"),
            P::number("tailLength", "Tail length", 1.0, 100.0, 1.0, 15.0).unit("px"),
            P::number("count", "Meteors per prop", 1.0, 20.0, 1.0, 1.0),
            direction(),
            P::boolean("sparkleTail", "Sparkling tail", true),
        ],
        EffectKind::Strobe => vec![
            P::color("color", "Color", "#ffffff"),
            P::number("rate", "Flashes per second", 0.5, 20.0, 0.5, 4.0).unit("Hz"),
            P::number("duty", "Flash length", 0.02, 0.9, 0.01, 0.15)
                .help("Share of each cycle the lights are on."),
            P::select("pattern", "Pattern", &["all", "random", "alternate"], "all"),
        ],
        EffectKind::Breathe => vec![
            P::colors("colors", "Colors", &["#ff0000", "#00c000"])
                .help("Each breath uses the next colour."),
            P::number("period", "Breath length", 0.5, 20.0, 0.1, 4.0).unit("s"),
            P::number("minBrightness", "Lowest brightness", 0.0, 1.0, 0.01, 0.05),
        ],
    };
    v.push(brightness());
    v
}

/// Complete and validate `params` against the schema for `kind`.
pub fn resolve_params(kind: EffectKind, params: &EffectParams) -> EffectParams {
    param_schema(kind)
        .into_iter()
        .map(|spec| {
            let value = params
                .get(&spec.key)
                .and_then(|v| spec.coerce(v))
                .unwrap_or_else(|| spec.default.clone());
            (spec.key, value)
        })
        .collect()
}

/// Typed read access to a resolved parameter map.
pub(crate) struct Params<'a>(pub(crate) &'a EffectParams);

impl Params<'_> {
    pub(crate) fn num(&self, key: &str) -> f32 {
        self.0
            .get(key)
            .and_then(Value::as_f64)
            .map_or(0.0, |v| v as f32)
    }

    pub(crate) fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(false)
    }

    pub(crate) fn color(&self, key: &str) -> Rgb {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .and_then(Rgb::from_hex)
            .unwrap_or(Rgb::WHITE)
    }

    pub(crate) fn colors(&self, key: &str) -> Vec<Rgb> {
        let list: Vec<Rgb> = self
            .0
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().and_then(Rgb::from_hex))
                    .collect()
            })
            .unwrap_or_default();
        if list.is_empty() {
            vec![Rgb::WHITE]
        } else {
            list
        }
    }

    pub(crate) fn select(&self, key: &str) -> &str {
        self.0.get(key).and_then(Value::as_str).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::ALL_EFFECT_KINDS;

    #[test]
    fn schemas_are_self_consistent() {
        for kind in ALL_EFFECT_KINDS {
            let schema = param_schema(kind);
            let mut keys: Vec<_> = schema.iter().map(|s| s.key.as_str()).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), schema.len(), "{kind:?} has duplicate keys");
            for spec in &schema {
                // Every default must survive coercion unchanged.
                assert_eq!(
                    spec.coerce(&spec.default).as_ref(),
                    Some(&spec.default),
                    "{kind:?}.{}",
                    spec.key
                );
                if spec.kind == ParamKind::Number {
                    assert!(spec.min.unwrap() <= spec.max.unwrap());
                }
            }
        }
    }

    #[test]
    fn resolve_fills_clamps_and_drops() {
        let mut p = EffectParams::new();
        p.insert("speed".into(), json!(1000));
        p.insert("colors".into(), json!(["#ff0000", "bogus", "00ff00"]));
        p.insert("direction".into(), json!("sideways"));
        p.insert("fade".into(), json!("off"));
        p.insert("mystery".into(), json!(1));
        let r = resolve_params(EffectKind::Chase, &p);
        assert_eq!(r["speed"], json!(60.0));
        assert_eq!(r["colors"], json!(["#ff0000", "#00ff00"]));
        assert_eq!(r["direction"], json!("forward"));
        assert_eq!(r["fade"], json!(false));
        assert_eq!(r["brightness"], json!(100.0));
        assert!(!r.contains_key("mystery"));
    }

    #[test]
    fn resolve_handles_odd_types() {
        let mut p = EffectParams::new();
        p.insert("color".into(), json!(["#123456"]));
        p.insert("brightness".into(), json!("55"));
        let r = resolve_params(EffectKind::Solid, &p);
        assert_eq!(r["color"], json!("#123456"));
        assert_eq!(r["brightness"], json!(55.0));

        let mut p = EffectParams::new();
        p.insert("colors".into(), json!("#abc"));
        p.insert("period".into(), json!(null));
        let r = resolve_params(EffectKind::Breathe, &p);
        assert_eq!(r["colors"], json!(["#aabbcc"]));
        assert_eq!(r["period"], json!(4.0));
    }

    #[test]
    fn schema_serializes_camel_case() {
        let v = serde_json::to_value(param_schema(EffectKind::Wave)).unwrap();
        let dir = v
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "direction")
            .unwrap();
        assert_eq!(dir["kind"], "select");
        assert!(dir["options"].as_array().unwrap().len() >= 4);
        assert!(dir.get("min").is_none());
        let speed = &v[1];
        assert_eq!(speed["kind"], "number");
        assert_eq!(speed["unit"], "waves/s");
    }
}
