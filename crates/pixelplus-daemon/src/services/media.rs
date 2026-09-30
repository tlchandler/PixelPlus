//! Audio files: analysis (duration, EBU R128 loudness, waveform peaks), the
//! per-file `media/<id>.meta.json`, loudness gain, transcoding and a short-lived
//! trash so "Undo" after a delete can bring files back.
//!
//! Analysis decodes with symphonia and measures with the `ebur128` crate (pure
//! Rust, works everywhere); if symphonia can't decode a file and `ffmpeg` is
//! installed, ffmpeg's `ebur128` filter is used instead.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Waveform resolution stored in the meta file.
pub const PEAK_BINS: usize = 1000;
/// Audio file extensions accepted for upload.
pub const AUDIO_EXTS: &[&str] = &["mp3", "ogg", "oga", "m4a", "aac", "wav", "flac"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaMeta {
    #[serde(default)]
    pub original_name: String,
    pub duration_ms: u64,
    #[serde(default)]
    pub loudness_lufs: Option<f32>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channels: Option<u32>,
    /// Normalized 0..1 peak per bin.
    #[serde(default)]
    pub peaks: Vec<f32>,
}

pub fn meta_path(media_dir: &Path, id: &str) -> PathBuf {
    media_dir.join(format!("{id}.meta.json"))
}

pub fn read_meta(media_dir: &Path, id: &str) -> Option<MediaMeta> {
    serde_json::from_slice(&std::fs::read(meta_path(media_dir, id)).ok()?).ok()
}

pub fn write_meta(media_dir: &Path, id: &str, meta: &MediaMeta) -> std::io::Result<()> {
    std::fs::write(meta_path(media_dir, id), serde_json::to_vec(meta)?)
}

/// Gain (dB) that brings `lufs` to `target`, limited to a sane range.
pub fn gain_for(target: f32, lufs: Option<f32>) -> Option<f32> {
    let l = lufs?;
    Some(((target - l).clamp(-20.0, 12.0) * 10.0).round() / 10.0)
}

/// Lower-case extension if it is an accepted audio type.
pub fn audio_ext(filename: &str) -> Option<String> {
    let ext = Path::new(filename)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    AUDIO_EXTS.contains(&ext.as_str()).then_some(ext)
}

/// "02_Wizards_in_Winter.mp3" -> "02 Wizards in Winter".
pub fn display_name(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let s: String = stem.replace(['_'], " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        "Untitled".into()
    } else {
        s
    }
}

/// Comparison key for auto-linking: lower-case letters and digits only.
pub fn match_key(name: &str) -> String {
    let stem = Path::new(name.trim())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    // Only strip a real audio/sequence extension (names may contain dots).
    let base = if Path::new(name.trim())
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()) || e.eq_ignore_ascii_case("fseq")
        }) {
        stem
    } else {
        name.trim()
    };
    base.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Analyze an audio file (blocking; run in `spawn_blocking`).
pub fn analyze(path: &Path) -> Result<MediaMeta, String> {
    match analyze_symphonia(path) {
        Ok(m) => Ok(m),
        Err(e) => {
            if super::system::have("ffmpeg") {
                analyze_ffmpeg(path).map_err(|e2| format!("{e}; ffmpeg: {e2}"))
            } else {
                Err(e)
            }
        }
    }
}

fn analyze_symphonia(path: &Path) -> Result<MediaMeta, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SErr;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("can't open the file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("not a supported audio format ({e})"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("the file has no audio track")?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported audio codec ({e})"))?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let mut meter: Option<ebur128::EbuR128> = None;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut frames: u64 = 0;
    // Peak per 50 ms window.
    let mut windows: Vec<f32> = Vec::new();
    let mut win_peak = 0f32;
    let mut win_len = 0u64;
    let mut win_size = 0u64;
    let mut errors = 0;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SErr::ResetRequired) => break,
            Err(e) => {
                if frames > 0 {
                    break;
                }
                return Err(format!("couldn't read the audio ({e})"));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SErr::DecodeError(_)) => {
                errors += 1;
                if errors > 1000 {
                    return Err("the audio is damaged".into());
                }
                continue;
            }
            Err(e) => return Err(format!("couldn't decode the audio ({e})")),
        };
        let spec = *decoded.spec();
        if meter.is_none() {
            rate = spec.rate;
            channels = spec.channels.count();
            meter = ebur128::EbuR128::new(channels as u32, rate, ebur128::Mode::I).ok();
            win_size = u64::from(rate / 20).max(1);
        }
        let dur = decoded.capacity() as u64;
        let buf = sample_buf.get_or_insert_with(|| SampleBuffer::<f32>::new(dur.max(4096), spec));
        if (buf.capacity() as u64) < dur * channels as u64 {
            *buf = SampleBuffer::<f32>::new(dur, spec);
        }
        buf.copy_interleaved_ref(decoded);
        let samples = buf.samples();
        if let Some(m) = meter.as_mut() {
            let _ = m.add_frames_f32(samples);
        }
        let ch = channels.max(1);
        for frame in samples.chunks_exact(ch) {
            let p = frame.iter().fold(0f32, |a, s| a.max(s.abs()));
            win_peak = win_peak.max(p);
            win_len += 1;
            if win_len >= win_size {
                windows.push(win_peak);
                win_peak = 0.0;
                win_len = 0;
            }
        }
        frames += (samples.len() / ch) as u64;
    }
    if win_len > 0 {
        windows.push(win_peak);
    }
    if frames == 0 || rate == 0 {
        return Err("the file contains no audio".into());
    }
    let loudness = meter
        .and_then(|m| m.loudness_global().ok())
        .filter(|l| l.is_finite())
        .map(|l| ((l * 10.0).round() / 10.0) as f32);
    Ok(MediaMeta {
        original_name: String::new(),
        duration_ms: frames * 1000 / u64::from(rate),
        loudness_lufs: loudness,
        sample_rate: Some(rate),
        channels: Some(channels as u32),
        peaks: resample_peaks(&windows, PEAK_BINS),
    })
}

/// Reduce/expand window peaks to `n` bins (max per bin), normalized to 0..1.
pub fn resample_peaks(windows: &[f32], n: usize) -> Vec<f32> {
    if windows.is_empty() || n == 0 {
        return vec![];
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = i * windows.len() / n;
        let b = (((i + 1) * windows.len()) / n)
            .max(a + 1)
            .min(windows.len());
        let m = windows[a.min(windows.len() - 1)..b]
            .iter()
            .fold(0f32, |x, y| x.max(*y));
        out.push(m);
    }
    let max = out.iter().fold(0f32, |a, b| a.max(*b));
    if max > 0.0 {
        for v in &mut out {
            *v = ((*v / max) * 1000.0).round() / 1000.0;
        }
    }
    out
}

/// ffmpeg input arguments for an uploaded file: local files only (no HLS /
/// concat / network tricks inside a crafted file), with the demuxer chosen
/// from the extension when known.
pub fn ffmpeg_input(path: &Path) -> Vec<String> {
    let mut args = vec!["-protocol_whitelist".to_string(), "file,pipe".to_string()];
    let demuxer = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .and_then(|e| match e.as_str() {
            "mp3" => Some("mp3"),
            "ogg" | "oga" => Some("ogg"),
            "m4a" => Some("mov"),
            "aac" => Some("aac"),
            "wav" => Some("wav"),
            "flac" => Some("flac"),
            _ => None,
        });
    if let Some(f) = demuxer {
        args.extend(["-f".to_string(), f.to_string()]);
    }
    args.extend(["-i".to_string(), format!("file:{}", path.to_string_lossy())]);
    args
}

fn analyze_ffmpeg(path: &Path) -> Result<MediaMeta, String> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats"])
        .args(ffmpeg_input(path))
        .args(["-af", "ebur128", "-f", "null", "-"])
        .output()
        .map_err(|e| format!("couldn't run ffmpeg: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err("ffmpeg couldn't read the file".into());
    }
    let loudness = parse_ffmpeg_loudness(&stderr);
    let duration_ms = parse_ffmpeg_duration(&stderr).ok_or("ffmpeg couldn't find the duration")?;
    Ok(MediaMeta {
        duration_ms,
        loudness_lufs: loudness,
        ..Default::default()
    })
}

#[cfg(test)]
#[test]
fn ffmpeg_inputs_are_local_files_only() {
    let a = ffmpeg_input(Path::new("/var/lib/pixelplus/media/x.M4A"));
    assert_eq!(
        a,
        [
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-i",
            "file:/var/lib/pixelplus/media/x.M4A"
        ]
    );
    let a = ffmpeg_input(Path::new("/tmp/upload.tmp"));
    assert_eq!(
        a,
        [
            "-protocol_whitelist",
            "file,pipe",
            "-i",
            "file:/tmp/upload.tmp"
        ]
    );
}

/// `I:  -16.3 LUFS` from the ebur128 summary.
pub fn parse_ffmpeg_loudness(stderr: &str) -> Option<f32> {
    let summary = stderr.rsplit("Summary:").next()?;
    summary.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("I:")?;
        rest.split_whitespace()
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
    })
}

/// `Duration: 00:03:25.43,` from ffmpeg's input description.
pub fn parse_ffmpeg_duration(stderr: &str) -> Option<u64> {
    let idx = stderr.find("Duration: ")?;
    let t = stderr[idx + 10..].split(',').next()?.trim();
    let mut parts = t.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(((h * 3600.0 + m * 60.0 + s) * 1000.0).round() as u64)
}

/// Transcode `src` to MP3 (44.1 kHz stereo, 192 kbit/s) with ffmpeg. `Ok(None)` when ffmpeg is missing.
pub async fn transcode_mp3(src: &Path, dst: &Path) -> Result<Option<()>, String> {
    if !super::system::have("ffmpeg") {
        return Ok(None);
    }
    let input = ffmpeg_input(src);
    let d = format!("file:{}", dst.to_string_lossy());
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error", "-y"];
    args.extend(input.iter().map(String::as_str));
    args.extend([
        "-ar",
        "44100",
        "-ac",
        "2",
        "-codec:a",
        "libmp3lame",
        "-b:a",
        "192k",
        &d,
    ]);
    let out = super::system::run("ffmpeg", &args, Duration::from_secs(300)).await?;
    if out.success {
        Ok(Some(()))
    } else {
        Err(format!(
            "ffmpeg couldn't convert the audio: {}",
            out.stderr.trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// Trash (undo support)
// ---------------------------------------------------------------------------

/// How long deleted files are kept for undo.
pub const TRASH_KEEP: Duration = Duration::from_secs(60 * 60);

pub fn trash_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(".trash")
}

/// Move `rel` (relative to the data dir) into the trash. Missing files are ignored.
pub fn trash(data_dir: &Path, rel: &str) {
    // Paths come from show.json: only ever well-formed data files.
    if super::paths::check(rel).is_none() {
        return;
    }
    let src = data_dir.join(rel);
    if !src.exists() {
        return;
    }
    let dir = trash_dir(data_dir);
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(rel.replace('/', "__"));
    if std::fs::rename(&src, &dst).is_err() {
        let _ = std::fs::remove_file(&src);
    } else {
        // Touch so the age check measures time since deletion.
        let _ = std::fs::File::options()
            .append(true)
            .open(&dst)
            .and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
    purge_trash(data_dir);
}

/// Bring `rel` back from the trash. Returns true if it was there.
pub fn untrash(data_dir: &Path, rel: &str) -> bool {
    if super::paths::check(rel).is_none() {
        return false;
    }
    let src = trash_dir(data_dir).join(rel.replace('/', "__"));
    if !src.exists() {
        return false;
    }
    if let Some(parent) = data_dir.join(rel).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::rename(&src, data_dir.join(rel)).is_ok()
}

/// Move `src` to `dst` (rename; copy + delete across file systems, e.g. an
/// upload directory on another mount). `dst`'s directory is created.
pub async fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(dir) = dst.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    if tokio::fs::rename(src, dst).await.is_ok() {
        return Ok(());
    }
    let tmp = dst.with_extension("moving.tmp");
    let res = async {
        tokio::fs::copy(src, &tmp).await?;
        tokio::fs::rename(&tmp, dst).await
    }
    .await;
    if res.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return res;
    }
    tokio::fs::remove_file(src).await
}

/// Temporary files older than this are leftovers (an upload still in
/// progress keeps writing to its file).
pub const STALE_TEMP: Duration = Duration::from_secs(60 * 60);

/// Remove leftovers of interrupted uploads and snapshot writes: a phone that
/// loses Wi-Fi halfway through a 300 MB sequence leaves `.upload-*` behind
/// (the request is dropped, so its own clean-up never runs).
pub fn purge_stale_temp(data_dir: &Path) {
    // (directory, is this file name a temporary file?)
    let upload = |n: &str| n.starts_with(".upload-");
    let dot_tmp = |n: &str| n.starts_with('.') && n.ends_with(".tmp");
    let any = |_: &str| true;
    type IsTemp<'a> = &'a dyn Fn(&str) -> bool;
    let dirs: [(PathBuf, IsTemp); 5] = [
        (data_dir.join("sequences"), &upload),
        (data_dir.join("media"), &upload),
        (data_dir.join("snapshots"), &dot_tmp),
        (data_dir.join("games").join("roms"), &dot_tmp),
        // xLights layouts being read for an import preview.
        (data_dir.join(".import"), &any),
    ];
    for (dir, is_temp) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let temp = is_temp(&name) && e.file_type().is_ok_and(|t| t.is_file());
            if !temp {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP);
            if old {
                tracing::info!("removing the leftover of an interrupted upload: {name}");
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

pub fn purge_trash(data_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(trash_dir(data_dir)) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map_or(true, |age| age > TRASH_KEEP);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A mono 16-bit WAV with a sine tone (for tests elsewhere too).
    pub fn sine_wav(path: &Path, seconds: f32, amplitude: f32) {
        let rate = 44_100u32;
        let n = (seconds * rate as f32) as u32;
        let mut data = Vec::with_capacity(n as usize * 2);
        for i in 0..n {
            let v = (amplitude
                * (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin()
                * 32767.0) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 2).to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(path, f).unwrap();
    }

    #[test]
    fn analyzes_wav() {
        let dir =
            std::env::temp_dir().join(format!("pp-media-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tone.wav");
        sine_wav(&p, 3.0, 0.5);
        let m = analyze(&p).unwrap();
        assert!((2990..=3010).contains(&m.duration_ms), "{}", m.duration_ms);
        let l = m.loudness_lufs.unwrap();
        // A -6 dBFS 440 Hz sine is about -9 LUFS (mono).
        assert!((-12.0..-6.0).contains(&l), "{l}");
        assert_eq!(m.peaks.len(), PEAK_BINS);
        assert_eq!(gain_for(-16.0, Some(l)).map(|g| g < 0.0), Some(true));
        std::fs::write(dir.join("junk.mp3"), b"not audio at all").unwrap();
        assert!(analyze_symphonia(&dir.join("junk.mp3")).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn names_and_keys() {
        assert_eq!(
            display_name("02_Wizards_in_Winter.mp3"),
            "02 Wizards in Winter"
        );
        assert_eq!(match_key("Wizards In Winter.MP3"), "wizardsinwinter");
        assert_eq!(
            match_key("C:\\x\\Mr. Sandman"),
            match_key("c:\\x\\mr sandman")
        );
        assert_eq!(audio_ext("a.FLAC").as_deref(), Some("flac"));
        assert_eq!(audio_ext("a.txt"), None);
    }

    #[test]
    fn ffmpeg_parsing() {
        let s = "  Duration: 00:03:25.43, start: 0\n...\n[Parsed_ebur128_0 @ 0x] Summary:\n\n  Integrated loudness:\n    I:         -14.2 LUFS\n";
        assert_eq!(parse_ffmpeg_loudness(s), Some(-14.2));
        assert_eq!(parse_ffmpeg_duration(s), Some(205_430));
    }

    #[test]
    fn peaks_resample() {
        let p = resample_peaks(&[0.1, 0.5, 0.2, 1.0], 2);
        assert_eq!(p, vec![0.5, 1.0]);
        assert_eq!(resample_peaks(&[0.5], 3), vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn stale_temp_files_are_removed() {
        let dir =
            std::env::temp_dir().join(format!("pp-stale-{}", pixelplus_core::model::new_id()));
        for d in ["sequences", "media", "snapshots"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let old = std::time::SystemTime::now() - STALE_TEMP - Duration::from_secs(60);
        let make = |rel: &str, aged: bool| {
            let p = dir.join(rel);
            std::fs::write(&p, b"partial").unwrap();
            if aged {
                std::fs::File::options()
                    .append(true)
                    .open(&p)
                    .unwrap()
                    .set_modified(old)
                    .unwrap();
            }
            p
        };
        let dead_seq = make("sequences/.upload-abc.fseq", true);
        let dead_audio = make("media/.upload-abc.mp3", true);
        let dead_snap = make("snapshots/.20261201-daily.tmp", true);
        std::fs::create_dir_all(dir.join(".import")).unwrap();
        let dead_import = make(".import/k3j2h1g4f5", true);
        let live = make("sequences/.upload-def.fseq", false);
        let real_seq = make("sequences/abc.fseq", true);
        let real_snap = make("snapshots/20261201-daily.tar.zst", true);
        purge_stale_temp(&dir);
        assert!(!dead_seq.exists() && !dead_audio.exists() && !dead_snap.exists());
        assert!(!dead_import.exists());
        assert!(live.exists(), "an upload in progress stays");
        assert!(real_seq.exists() && real_snap.exists(), "data files stay");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn trash_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("pp-trash-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/a.mp3"), b"x").unwrap();
        trash(&dir, "media/a.mp3");
        assert!(!dir.join("media/a.mp3").exists());
        assert!(untrash(&dir, "media/a.mp3"));
        assert!(dir.join("media/a.mp3").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
