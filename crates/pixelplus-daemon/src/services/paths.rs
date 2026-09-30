//! Data-file paths stored in `show.json` are never trusted (ARCHITECTURE §6).
//!
//! A show can come from anywhere (a snapshot shared online, an older
//! version, a hand-edited file), so every `media.file`, `sequence.file` and
//! `sequence.thumbnail` must have the form `<dir>/<name>.<ext>`:
//!
//! | dir | name | ext |
//! |---|---|---|
//! | `media/` | `[A-Za-z0-9_-]{1,64}` | audio ([`AUDIO_EXTS`]) or `meta.json` |
//! | `sequences/` | same | `fseq`, `ppseq` |
//! | `thumbnails/` | same | `png` |
//!
//! [`sanitize_show`] runs on every store write and on load: a path that isn't
//! of that form is rebuilt from the entity id (`media/<id>.<ext>`) or blanked.
//! Use sites that touch the file system still go through [`resolve`].

use super::media::AUDIO_EXTS;
use pixelplus_core::model::Show;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Media,
    MediaMeta,
    Sequence,
    Thumbnail,
}

/// A file-name-safe id (no dots, slashes or anything else).
pub fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What kind of data file `rel` is, if it is a well-formed data path.
pub fn check(rel: &str) -> Option<Kind> {
    let (dir, file) = rel.split_once('/')?;
    if file.contains('/') {
        return None;
    }
    if dir == "media" {
        if let Some(name) = file.strip_suffix(".meta.json") {
            return safe_name(name).then_some(Kind::MediaMeta);
        }
    }
    let (name, ext) = file.split_once('.')?;
    if !safe_name(name) {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    match dir {
        "media" if AUDIO_EXTS.contains(&ext.as_str()) => Some(Kind::Media),
        "sequences" if ext == "fseq" || ext == "ppseq" => Some(Kind::Sequence),
        "thumbnails" if ext == "png" => Some(Kind::Thumbnail),
        _ => None,
    }
}

/// `data_dir/rel` when `rel` is a well-formed data path of `kind`.
pub fn resolve(data_dir: &Path, rel: &str, kind: Kind) -> Option<PathBuf> {
    (check(rel) == Some(kind)).then(|| data_dir.join(rel))
}

/// Lower-case extension of the last path component (`"x/y.MP3"` → `"mp3"`).
fn ext_of(rel: &str) -> Option<String> {
    let file = rel.rsplit(['/', '\\']).next()?;
    let (_, ext) = file.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

/// Make every data path in `show` well-formed (see module docs). Returns a
/// description of each change.
pub fn sanitize_show(show: &mut Show) -> Vec<String> {
    let mut changes = Vec::new();
    for m in show.media.iter_mut() {
        if m.file.is_empty() || check(&m.file) == Some(Kind::Media) {
            continue;
        }
        let fixed = ext_of(&m.file)
            .filter(|e| AUDIO_EXTS.contains(&e.as_str()) && safe_name(&m.id))
            .map(|e| format!("media/{}.{e}", m.id))
            .unwrap_or_default();
        changes.push(format!("audio “{}”: {:?} → {:?}", m.name, m.file, fixed));
        m.file = fixed;
    }
    for s in show.sequences.iter_mut() {
        if !s.file.is_empty() && check(&s.file) != Some(Kind::Sequence) {
            let fixed = ext_of(&s.file)
                .filter(|e| (e == "fseq" || e == "ppseq") && safe_name(&s.id))
                .map(|e| format!("sequences/{}.{e}", s.id))
                .unwrap_or_default();
            changes.push(format!("sequence “{}”: {:?} → {:?}", s.name, s.file, fixed));
            s.file = fixed;
        }
        if let Some(t) = &s.thumbnail {
            if check(t) != Some(Kind::Thumbnail) {
                let fixed = safe_name(&s.id).then(|| format!("thumbnails/{}.png", s.id));
                changes.push(format!("thumbnail of “{}”: {t:?} → {fixed:?}", s.name));
                s.thumbnail = fixed;
            }
        }
    }
    for c in &changes {
        tracing::warn!("unsafe file path in the show replaced: {c}");
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::{Media, MediaKind, Sequence};

    #[test]
    fn well_formed_paths() {
        assert_eq!(check("media/ab12.mp3"), Some(Kind::Media));
        assert_eq!(check("media/ab12.MP3"), Some(Kind::Media));
        assert_eq!(check("media/ab12.meta.json"), Some(Kind::MediaMeta));
        assert_eq!(check("sequences/s1.fseq"), Some(Kind::Sequence));
        assert_eq!(check("sequences/not-there.fseq"), Some(Kind::Sequence));
        assert_eq!(check("thumbnails/s1.png"), Some(Kind::Thumbnail));
        for bad in [
            "",
            "node.json",
            "/etc/passwd",
            "media/../node.json",
            "media/x.html",
            "media/x.svg",
            "media/.mp3",
            "media/a.b.mp3",
            "media/sub/x.mp3",
            "sequences/x.png",
            "thumbnails/x.png.exe",
            "../media/x.mp3",
            "media\\..\\x.mp3",
            "dj/x.mp3",
        ] {
            assert_eq!(check(bad), None, "{bad}");
        }
        assert!(resolve(Path::new("/d"), "media/a.mp3", Kind::Media).is_some());
        assert!(resolve(Path::new("/d"), "media/a.mp3", Kind::Thumbnail).is_none());
    }

    #[test]
    fn hostile_show_paths_are_rebuilt_from_ids() {
        let mut show = Show::default();
        show.media.push(Media {
            id: "m1".into(),
            name: "Song".into(),
            kind: MediaKind::Song,
            file: "/etc/../var/lib/pixelplus/x.MP3".into(),
            duration_ms: 1,
            loudness_lufs: None,
            gain_db: None,
        });
        show.media.push(Media {
            id: "m2".into(),
            name: "Page".into(),
            kind: MediaKind::Song,
            file: "media/m2.html".into(),
            duration_ms: 1,
            loudness_lufs: None,
            gain_db: None,
        });
        show.media.push(Media {
            id: "../evil".into(),
            name: "Bad id".into(),
            kind: MediaKind::Song,
            file: "node.json.mp3".into(),
            duration_ms: 1,
            loudness_lufs: None,
            gain_db: None,
        });
        show.sequences.push(Sequence {
            id: "s1".into(),
            name: "S".into(),
            file: "../../node.json".into(),
            duration_ms: 1,
            frame_ms: 25,
            channel_count: 3,
            media_id: None,
            xlights_name: None,
            thumbnail: Some("/tmp/owned.png".into()),
            hash: String::new(),
        });
        let changes = sanitize_show(&mut show);
        assert_eq!(changes.len(), 5);
        assert_eq!(show.media[0].file, "media/m1.mp3");
        assert_eq!(show.media[1].file, "", "not audio: unusable");
        assert_eq!(show.media[2].file, "", "unsafe id: unusable");
        assert_eq!(show.sequences[0].file, "");
        assert_eq!(
            show.sequences[0].thumbnail.as_deref(),
            Some("thumbnails/s1.png")
        );
        // Idempotent, and well-formed shows are untouched.
        assert!(sanitize_show(&mut show).is_empty());
    }
}
