//! First-run content: the built-in DJ voices (Nick & Holly, from
//! tlchandler/fpp-voices `voices.json`), the tested pronunciation fixes
//! (`pronunciations.txt`), the built-in looks and a default playlist.

use pixelplus_core::model::{DjVoice, Playlist, Pronunciation, Show};
use std::collections::BTreeMap;

const PRONUNCIATIONS_TXT: &str = include_str!("../../assets/pronunciations.txt");

fn map(pairs: &[(&str, f32)]) -> BTreeMap<String, f32> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Nick & Holly, exactly as tuned in fpp-voices.
pub fn builtin_voices() -> Vec<DjVoice> {
    vec![
        DjVoice {
            id: "nick".into(),
            name: "Nick".into(),
            description: "Male DJ - warm, upbeat, classic radio baritone".into(),
            blend: map(&[("am_echo", 0.3), ("am_fenrir", 0.3), ("am_puck", 0.4)]),
            speed: 1.05,
            lang: "en-us".into(),
            eq: Some("equalizer=f=150:t=q:w=1.0:g=2.5,equalizer=f=3200:t=q:w=1.2:g=2".into()),
            default_energy: 0.4,
            energy: map(&[
                ("pitch", 1.5),
                ("range", 1.5),
                ("speed", 1.0),
                ("stretch", 1.0),
                ("boost", 3.0),
                ("lift", 3.0),
                ("ceiling", 3.0),
                ("maxLift", 9.0),
            ]),
        },
        DjVoice {
            id: "holly".into(),
            name: "Holly".into(),
            description: "Female DJ - bright, friendly, energetic".into(),
            blend: map(&[("af_heart", 0.5), ("af_kore", 0.5)]),
            speed: 1.05,
            lang: "en-us".into(),
            eq: Some("equalizer=f=220:t=q:w=1.0:g=1.5,equalizer=f=4000:t=q:w=1.2:g=2".into()),
            default_energy: 0.4,
            energy: map(&[
                ("pitch", 2.0),
                ("range", 1.5),
                ("speed", 1.1),
                ("stretch", 1.05),
                ("boost", 3.0),
                ("lift", 2.5),
                ("ceiling", 3.0),
                ("maxLift", 8.0),
            ]),
        },
    ]
}

/// Parse fpp-voices' `pronunciations.txt` (`word = replacement`, `# ` comments).
pub fn parse_pronunciations(text: &str) -> Vec<Pronunciation> {
    let mut out: Vec<Pronunciation> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((word, say)) = line.split_once('=') else {
            continue;
        };
        let (word, say) = (word.trim(), say.trim());
        if word.is_empty() || say.is_empty() || out.iter().any(|p| p.word == word) {
            continue;
        }
        out.push(Pronunciation { word: word.into(), say: say.into() });
    }
    out
}

pub fn builtin_pronunciations() -> Vec<Pronunciation> {
    parse_pronunciations(PRONUNCIATIONS_TXT)
}

/// Add whatever built-in content is missing. Never overwrites user data.
pub fn seed_defaults(show: &mut Show) {
    for v in builtin_voices() {
        if !show.dj_voices.iter().any(|x| x.id == v.id) {
            show.dj_voices.push(v);
        }
    }
    if show.pronunciations.is_empty() {
        show.pronunciations = builtin_pronunciations();
    }
    if !show.effects.iter().any(|e| e.id.starts_with("builtin-")) {
        for e in pixelplus_core::effects::builtin_presets() {
            if !show.effects.iter().any(|x| x.id == e.id) {
                show.effects.push(e);
            }
        }
    }
    if show.playlists.is_empty() {
        show.playlists.push(Playlist {
            id: pixelplus_core::model::new_id(),
            name: "Main Show".into(),
            items: vec![],
            intro: vec![],
            outro: vec![],
            shuffle: false,
            repeat: true,
            crossfade_ms: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pronunciations_parse() {
        let p = builtin_pronunciations();
        assert!(p.len() > 50, "{}", p.len());
        let noel = p.iter().find(|p| p.word == "Noel").unwrap();
        assert_eq!(noel.say, "/noʊˈɛl/");
        assert!(p.iter().all(|p| !p.word.starts_with('#')));
    }

    #[test]
    fn seeding_is_idempotent() {
        let mut s = Show::default();
        seed_defaults(&mut s);
        let snapshot = s.clone();
        seed_defaults(&mut s);
        assert_eq!(s, snapshot);
        assert_eq!(s.dj_voices.len(), 2);
        assert_eq!(s.playlists[0].name, "Main Show");
        assert!(s.effects.iter().any(|e| e.id.starts_with("builtin-")));
        assert_eq!(s.dj_voices[0].energy["maxLift"], 9.0);
    }
}
