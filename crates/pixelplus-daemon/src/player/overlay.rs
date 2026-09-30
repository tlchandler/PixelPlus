//! Prop overlays (ARCHITECTURE §10): live content that temporarily replaces a
//! prop's pixels — games (shared memory), HTTP frames, scrolling text, QR
//! codes, and frames forwarded from the leader to followers.
//!
//! Overlays are composited after the sequence/effect and before the output
//! pipeline. An overlay keeps showing its last frame while it is enabled.
//!
//! Shared memory: `/dev/shm/pixelplus-overlay-<propId>` = three native-endian
//! u32 (width, height, flags) + width×height×bpp bytes, row-major from the
//! top-left. Flags bit 0 = "new frame" (set by the writer, cleared by us after
//! copying); bits 8–15 = bytes per pixel (0 means 3; 4 is accepted, the 4th
//! byte is ignored). The file is created mode 0660: the games sidecar runs as
//! the same `pixelplus` service user (or a member of its group) - see
//! packaging/systemd/pixelplus-games.service and docker/docker-compose.yml.

use super::OverlayInfo;
use pixelplus_core::effects::Rgb;
use pixelplus_core::model::{MatrixInfo, Prop};
use pixelplus_core::text::{self, Font, QrStyle, RgbGrid};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Implicitly enabled overlays (frames pushed without `enable`) switch off
/// after this long without a new frame.
const AUTO_EXPIRY: Duration = Duration::from_secs(2);
/// Scrolling text speed.
const SCROLL_PX_PER_S: f32 = 12.0;
/// Header size of the shared-memory buffer.
pub const SHM_HEADER: usize = 12;

/// Path of a prop's shared-memory overlay buffer inside `dir`.
pub fn shm_path(dir: &Path, prop_id: &str) -> PathBuf {
    let safe: String = prop_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    dir.join(format!("pixelplus-overlay-{safe}"))
}

/// Grid geometry of a prop: its matrix, or a single row of its pixels.
pub fn prop_matrix(prop: &Prop) -> MatrixInfo {
    match &prop.matrix {
        Some(m) if m.width > 0 && m.height > 0 => m.clone(),
        _ => MatrixInfo {
            width: prop.pixel_count.max(1),
            height: 1,
            pixel_map: (0..prop.pixel_count as i32).collect(),
        },
    }
}

struct Shm {
    file: File,
    path: PathBuf,
}

enum TempKind {
    Text { text: String, color: Rgb, scroll: bool },
    Qr { grid: RgbGrid },
}

struct Temp {
    kind: TempKind,
    started: Instant,
    until: Option<Instant>,
    enabled_before: bool,
}

struct Overlay {
    matrix: MatrixInfo,
    pixel_count: usize,
    enabled: bool,
    /// Enabled implicitly by a pushed frame at this time (expires).
    auto_since: Option<Instant>,
    shm: Option<Shm>,
    /// Current content in prop pixel order.
    pixels: Vec<u8>,
    has_content: bool,
    temp: Option<Temp>,
    /// Content changed since the last forward to followers.
    dirty: bool,
    last_forward: Option<Instant>,
    last_frame: Instant,
}

impl Overlay {
    fn new(prop: &Prop) -> Self {
        Overlay {
            matrix: prop_matrix(prop),
            pixel_count: prop.pixel_count as usize,
            enabled: false,
            auto_since: None,
            shm: None,
            pixels: vec![0; prop.pixel_count as usize * 3],
            has_content: false,
            temp: None,
            dirty: false,
            last_forward: None,
            last_frame: Instant::now(),
        }
    }

    fn set_grid(&mut self, grid: &RgbGrid) {
        text::blit_grid_to_prop(grid, &self.matrix, &mut self.pixels);
        self.has_content = true;
        self.dirty = true;
    }

    fn pushed(&mut self, now: Instant) {
        self.last_frame = now;
        if !self.enabled {
            self.enabled = true;
            self.auto_since = Some(now);
        } else if self.auto_since.is_some() {
            self.auto_since = Some(now);
        }
    }
}

/// All overlays of this node.
pub struct OverlayManager {
    dir: PathBuf,
    overlays: HashMap<String, Overlay>,
}

impl OverlayManager {
    /// Shared-memory buffers are created in `dir` (normally `/dev/shm`).
    pub fn new(dir: PathBuf) -> Self {
        OverlayManager { dir, overlays: HashMap::new() }
    }

    fn entry(&mut self, prop: &Prop) -> &mut Overlay {
        let o = self.overlays.entry(prop.id.clone()).or_insert_with(|| Overlay::new(prop));
        // Geometry changed (prop edited): rebuild the mapping, keep state.
        if o.pixel_count != prop.pixel_count as usize || o.matrix != prop_matrix(prop) {
            o.matrix = prop_matrix(prop);
            o.pixel_count = prop.pixel_count as usize;
            o.pixels = vec![0; o.pixel_count * 3];
            o.shm = None;
        }
        o
    }

    /// Create (or reuse) the shared-memory buffer for `prop`.
    pub fn open(&mut self, prop: &Prop) -> Result<OverlayInfo, String> {
        let dir = self.dir.clone();
        let o = self.entry(prop);
        let (w, h) = (o.matrix.width, o.matrix.height);
        let path = shm_path(&dir, &prop.id);
        if o.shm.is_none() {
            o.shm = Some(create_shm(&path, w, h).map_err(|e| format!("could not create {}: {e}", path.display()))?);
        }
        Ok(OverlayInfo { shm: path.to_string_lossy().into_owned(), width: w, height: h })
    }

    pub fn enable(&mut self, prop: &Prop, enabled: bool) {
        let o = self.entry(prop);
        o.enabled = enabled;
        o.auto_since = None;
        o.dirty = true;
        if !enabled {
            o.temp = None;
        }
    }

    /// Replace the content with a grid frame (row-major RGB, width×height×3).
    pub fn set_frame(&mut self, prop: &Prop, rgb: &[u8], now: Instant) {
        let o = self.entry(prop);
        let (w, h) = (o.matrix.width, o.matrix.height);
        let mut data = rgb.to_vec();
        data.resize(w as usize * h as usize * 3, 0);
        if let Some(grid) = RgbGrid::from_bytes(w, h, data) {
            o.set_grid(&grid);
            o.temp = None;
            o.pushed(now);
        }
    }

    /// Replace the content with pixels in prop order (pixelCount×3).
    pub fn set_prop_pixels(&mut self, prop: &Prop, rgb: &[u8], now: Instant) {
        let o = self.entry(prop);
        let n = rgb.len().min(o.pixels.len());
        o.pixels[..n].copy_from_slice(&rgb[..n]);
        o.pixels[n..].fill(0);
        o.has_content = true;
        o.dirty = true;
        o.temp = None;
        o.pushed(now);
    }

    /// Show text (scrolling or fitted) for `duration_ms` (0 = until disabled).
    pub fn text(&mut self, prop: &Prop, text: &str, color: &str, scroll: bool, duration_ms: u64, now: Instant) {
        let color = parse_color(color);
        let o = self.entry(prop);
        let enabled_before = o.temp.as_ref().map_or(o.enabled && o.auto_since.is_none(), |t| t.enabled_before);
        o.temp = Some(Temp {
            kind: TempKind::Text { text: text.to_string(), color, scroll, },
            started: now,
            until: (duration_ms > 0).then(|| now + Duration::from_millis(duration_ms)),
            enabled_before,
        });
        o.enabled = true;
        o.auto_since = None;
    }

    /// Show a QR code for `duration_ms` (0 = until disabled).
    pub fn qr(&mut self, prop: &Prop, url: &str, duration_ms: u64, now: Instant) -> Result<(), String> {
        let o = self.entry(prop);
        let grid = text::render_qr(url, o.matrix.width, o.matrix.height, QrStyle::default()).map_err(|e| e.to_string())?;
        let enabled_before = o.temp.as_ref().map_or(o.enabled && o.auto_since.is_none(), |t| t.enabled_before);
        o.temp = Some(Temp {
            kind: TempKind::Qr { grid },
            started: now,
            until: (duration_ms > 0).then(|| now + Duration::from_millis(duration_ms)),
            enabled_before,
        });
        o.enabled = true;
        o.auto_since = None;
        Ok(())
    }

    /// Per output frame: read shared memory, animate text, expire overlays.
    pub fn update(&mut self, now: Instant) {
        for o in self.overlays.values_mut() {
            if let Some(t) = &o.temp {
                if t.until.is_some_and(|u| now >= u) {
                    o.enabled = t.enabled_before;
                    o.temp = None;
                    o.dirty = true;
                }
            }
            if let Some(since) = o.auto_since {
                if now.duration_since(since.max(o.last_frame)) > AUTO_EXPIRY {
                    o.enabled = false;
                    o.auto_since = None;
                }
            }
            if !o.enabled {
                continue;
            }
            if let Some(t) = &o.temp {
                let (w, h) = (o.matrix.width, o.matrix.height);
                let grid = match &t.kind {
                    TempKind::Text { text: s, color, scroll: true } => {
                        let font = Font::for_height(h);
                        let tw = text::text_width(s, font, 1);
                        let t_ms = now.duration_since(t.started).as_millis() as u64;
                        text::render_text(s, *color, w, h, text::marquee_offset(t_ms, SCROLL_PX_PER_S, tw, w))
                    }
                    TempKind::Text { text: s, color, scroll: false } => {
                        text::render_text_fit(s, Font::for_height(h), *color, w, h)
                    }
                    TempKind::Qr { grid } => grid.clone(),
                };
                o.pixels.fill(0);
                o.set_grid(&grid);
                continue;
            }
            if let Some(shm) = &o.shm {
                match poll_shm(&shm.file, o.matrix.width, o.matrix.height) {
                    Ok(Some(grid)) => {
                        o.set_grid(&grid);
                        o.last_frame = now;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!("overlay {}: {e}", shm.path.display());
                    }
                }
            }
        }
    }

    /// Any overlay currently showing?
    pub fn any_active(&self) -> bool {
        self.overlays.values().any(|o| o.enabled && (o.has_content || o.temp.is_some()))
    }

    /// Call `f(prop_id, pixels)` for every enabled overlay with content.
    pub fn for_each_active(&self, mut f: impl FnMut(&str, &[u8])) {
        for (id, o) in &self.overlays {
            if o.enabled && o.has_content {
                f(id, &o.pixels);
            }
        }
    }

    /// Overlays whose content should be sent to followers now (changed, or a
    /// 1 s keep-alive). Marks them as forwarded.
    pub fn take_forwards(&mut self, now: Instant, mut f: impl FnMut(&str, &[u8])) {
        for (id, o) in &mut self.overlays {
            if !(o.enabled && o.has_content) {
                continue;
            }
            let due = o.dirty || o.last_forward.is_none_or_older(now, Duration::from_secs(1));
            if due {
                f(id, &o.pixels);
                o.dirty = false;
                o.last_forward = Some(now);
            }
        }
    }

    /// Drop overlays of props that no longer exist.
    pub fn retain_props(&mut self, exists: impl Fn(&str) -> bool) {
        self.overlays.retain(|id, _| exists(id));
    }
}

trait OlderThan {
    fn is_none_or_older(&self, now: Instant, age: Duration) -> bool;
}

impl OlderThan for Option<Instant> {
    fn is_none_or_older(&self, now: Instant, age: Duration) -> bool {
        match self {
            None => true,
            Some(t) => now.duration_since(*t) >= age,
        }
    }
}

/// Parse `#rrggbb` (default white).
pub fn parse_color(s: &str) -> Rgb {
    Rgb::from_hex(s.trim()).unwrap_or(Rgb::WHITE)
}

/// Overlay buffers: read/write for the `pixelplus` user and the sidecar group only.
const SHM_MODE: u32 = 0o660;

fn create_shm(path: &Path, w: u32, h: u32) -> std::io::Result<Shm> {
    use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // /dev/shm is world-writable: never follow a link planted at our name.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(SHM_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    // Room for 4 bytes per pixel so either pixel format fits.
    let len = (SHM_HEADER + w as usize * h as usize * 4) as u64;
    if file.metadata()?.len() < len {
        file.set_len(len)?;
    }
    let mut header = [0u8; SHM_HEADER];
    header[0..4].copy_from_slice(&w.to_ne_bytes());
    header[4..8].copy_from_slice(&h.to_ne_bytes());
    header[8..12].copy_from_slice(&0u32.to_ne_bytes());
    file.write_all_at(&header, 0)?;
    // Also fixes files left by older versions (0666) - by descriptor, not by path.
    let _ = file.set_permissions(std::fs::Permissions::from_mode(SHM_MODE));
    // The games sidecar runs as its own user in the sidecar group
    // (pixelplus-overlay): give the buffer to that group.
    if let Some(gid) = crate::api::security::sidecar_gid() {
        use std::os::fd::AsRawFd;
        // SAFETY: fchown on our own open descriptor.
        let _ = unsafe { libc::fchown(file.as_raw_fd(), u32::MAX, gid) };
    }
    Ok(Shm { file, path: path.to_path_buf() })
}

/// Read a new frame if the writer flagged one; clears the flag.
fn poll_shm(file: &File, w: u32, h: u32) -> std::io::Result<Option<RgbGrid>> {
    use std::os::unix::fs::FileExt;
    let mut flags_b = [0u8; 4];
    file.read_exact_at(&mut flags_b, 8)?;
    let flags = u32::from_ne_bytes(flags_b);
    if flags & 1 == 0 {
        return Ok(None);
    }
    let bpp = match (flags >> 8) & 0xff {
        0 | 3 => 3usize,
        4 => 4,
        other => {
            file.write_all_at(&(flags & !1).to_ne_bytes(), 8)?;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported overlay pixel size {other}"),
            ));
        }
    };
    let n = w as usize * h as usize;
    let mut raw = vec![0u8; n * bpp];
    file.read_exact_at(&mut raw, SHM_HEADER as u64)?;
    file.write_all_at(&(flags & !1).to_ne_bytes(), 8)?;
    let rgb = if bpp == 3 {
        raw
    } else {
        raw.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
    };
    Ok(RgbGrid::from_bytes(w, h, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelplus_core::model::PropKind;
    use std::os::unix::fs::FileExt;

    fn matrix_prop() -> Prop {
        Prop {
            id: "mx".into(),
            name: "Matrix".into(),
            kind: PropKind::Matrix,
            pixel_count: 8,
            xlights_model: None,
            channel_start: 0,
            channels_per_pixel: 3,
            channel_runs: None,
            segments: vec![],
            group_ids: vec![],
            layout: None,
            // 4×2, serpentine: row 0 left→right = 0..3, row 1 right→left = 4..7.
            matrix: Some(MatrixInfo { width: 4, height: 2, pixel_map: vec![0, 1, 2, 3, 7, 6, 5, 4] }),
            color: None,
            max_milliamps_per_pixel: None,
            notes: None,
        }
    }

    /// A fresh path (created on demand by `open`).
    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("pp-ovl-{}", pixelplus_core::model::new_id()))
    }

    fn active(m: &OverlayManager) -> Option<Vec<u8>> {
        let mut out = None;
        m.for_each_active(|_, px| out = Some(px.to_vec()));
        out
    }

    #[test]
    fn shm_round_trip() {
        let dir = tmp();
        let prop = matrix_prop();
        let mut m = OverlayManager::new(dir.clone());
        let info = m.open(&prop).unwrap();
        assert_eq!((info.width, info.height), (4, 2));
        let path = PathBuf::from(&info.shm);
        let meta = std::fs::metadata(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o660);
        let f = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let mut hdr = [0u8; 12];
        f.read_exact_at(&mut hdr, 0).unwrap();
        assert_eq!(u32::from_ne_bytes(hdr[0..4].try_into().unwrap()), 4);
        assert_eq!(u32::from_ne_bytes(hdr[4..8].try_into().unwrap()), 2);

        // Writer: pixel (x, y) = (x*10, y*100, 7); then set flag bit 0.
        let mut frame = vec![];
        for y in 0..2u8 {
            for x in 0..4u8 {
                frame.extend_from_slice(&[x * 10, y * 100, 7]);
            }
        }
        f.write_all_at(&frame, 12).unwrap();
        f.write_all_at(&1u32.to_ne_bytes(), 8).unwrap();

        let now = Instant::now();
        m.update(now);
        assert!(active(&m).is_none(), "not enabled yet");
        m.enable(&prop, true);
        m.update(now);
        // The flag was consumed before enabling? No: disabled overlays are not polled.
        let px = active(&m).expect("frame copied");
        // Prop pixel 4 is at (3, 1).
        assert_eq!(&px[4 * 3..4 * 3 + 3], &[30, 100, 7]);
        assert_eq!(&px[0..3], &[0, 0, 7]);
        f.read_exact_at(&mut hdr, 0).unwrap();
        assert_eq!(u32::from_ne_bytes(hdr[8..12].try_into().unwrap()) & 1, 0, "flag cleared");

        // No new frame: the last frame stays.
        m.update(now);
        assert_eq!(active(&m).unwrap(), px);

        // 4 bytes per pixel.
        let mut frame4 = vec![];
        for _ in 0..8 {
            frame4.extend_from_slice(&[1, 2, 3, 99]);
        }
        f.write_all_at(&frame4, 12).unwrap();
        f.write_all_at(&(1u32 | (4 << 8)).to_ne_bytes(), 8).unwrap();
        m.update(now);
        assert_eq!(&active(&m).unwrap()[0..6], &[1, 2, 3, 1, 2, 3]);

        m.enable(&prop, false);
        assert!(active(&m).is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn pushed_frames_auto_enable_and_expire() {
        let mut m = OverlayManager::new(tmp());
        let prop = matrix_prop();
        let t0 = Instant::now();
        m.set_prop_pixels(&prop, &[9; 24], t0);
        m.update(t0);
        assert_eq!(active(&m).unwrap(), vec![9; 24]);
        m.update(t0 + Duration::from_secs(3));
        assert!(active(&m).is_none(), "expired");
        // Explicitly enabled overlays do not expire.
        m.enable(&prop, true);
        m.set_frame(&prop, &[5; 24], t0);
        m.update(t0 + Duration::from_secs(60));
        assert_eq!(active(&m).unwrap(), vec![5; 24]);
    }

    #[test]
    fn text_and_qr_are_temporary() {
        let mut m = OverlayManager::new(tmp());
        let mut prop = matrix_prop();
        prop.pixel_count = 32 * 16;
        prop.matrix = Some(MatrixInfo { width: 32, height: 16, pixel_map: (0..512).collect() });
        let t0 = Instant::now();
        m.text(&prop, "HI", "#ff0000", false, 1000, t0);
        m.update(t0);
        let px = active(&m).unwrap();
        assert!(px.chunks(3).any(|p| p == [255, 0, 0]), "red text drawn");
        assert!(px.chunks(3).all(|p| p == [255, 0, 0] || p == [0, 0, 0]));
        // Scrolling text changes over time.
        m.text(&prop, "HELLO", "#00ff00", true, 0, t0);
        m.update(t0 + Duration::from_millis(500));
        let a = active(&m).unwrap();
        m.update(t0 + Duration::from_millis(1500));
        let b = active(&m).unwrap();
        assert_ne!(a, b);
        m.enable(&prop, false);
        // QR (needs at least 21×21): too small here, fine on 32×32.
        assert!(m.qr(&prop, "http://x.y/", 1000, t0).is_err());
        prop.pixel_count = 32 * 32;
        prop.matrix = Some(MatrixInfo { width: 32, height: 32, pixel_map: (0..1024).collect() });
        m.qr(&prop, "http://x.y/", 1000, t0).unwrap();
        m.update(t0);
        assert!(active(&m).is_some());
        m.update(t0 + Duration::from_millis(1100));
        assert!(active(&m).is_none());
        // Forwarding: changed content is due; unchanged only after 1 s.
        m.set_prop_pixels(&prop, &[1; 12], t0);
        let mut n = 0;
        m.take_forwards(t0, |_, _| n += 1);
        m.take_forwards(t0 + Duration::from_millis(100), |_, _| n += 1);
        assert_eq!(n, 1);
        m.take_forwards(t0 + Duration::from_millis(1200), |_, _| n += 1);
        assert_eq!(n, 2);
    }

    #[test]
    fn linear_props_get_a_single_row() {
        let mut p = matrix_prop();
        p.matrix = None;
        let mi = prop_matrix(&p);
        assert_eq!((mi.width, mi.height), (8, 1));
        assert_eq!(mi.pixel_map, (0..8).collect::<Vec<_>>());
        assert_eq!(shm_path(Path::new("/dev/shm"), "a/b").to_str().unwrap(), "/dev/shm/pixelplus-overlay-a_b");
    }
}
