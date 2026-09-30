//! The DPI backend: WS281x through the Raspberry Pi display pipeline.
//!
//! PixelPlus drives DPI through **DRM/KMS** rather than `/dev/fb0`:
//!
//! * the DPI connector is found explicitly (`/dev/dri/card*`, connector type
//!   DPI), independent of which card or connector fbdev emulation picked;
//! * the video mode is read back from the kernel, so the encoder always
//!   matches the timing actually programmed by the overlay;
//! * two dumb buffers are page-flipped on vblank with completion events, so a
//!   frame is never shown half-written and pacing is exact.
//!
//! `write_frame` encodes into the back buffer while the front buffer is being
//! scanned out, then queues a flip. If the previous flip has not completed
//! yet it first waits for it (at most one refresh period), so call it from a
//! dedicated output thread.

use super::{OutputStats, PixelOutput};
use crate::encoder::{BufferState, FrameBufferMut, WsEncoder};
use crate::error::{OutputError, Result};
use crate::frame::{OutputFrame, OutputFrameRef};
use crate::layout::OutputLayout;
use crate::pi_config::DpiSoc;
use crate::pinmux::PinMux;
use crate::timing::DpiGeometry;
use drm::buffer::{Buffer as _, DrmFourcc};
use drm::Device as _;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice, Event, Mode,
    ModeTypeFlags, PageFlipFlags,
};
use pixelplus_core::model::BoardKind;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where the Pi publishes its model string.
pub const DEVICE_TREE_MODEL: &str = "/proc/device-tree/model";

/// Settings for [`DpiOutput`].
#[derive(Debug, Clone)]
pub struct DpiConfig {
    /// The board (decides the output layout and pins).
    pub board: BoardKind,
    /// DRM device to use; `None` scans `/dev/dri/card*` for a DPI connector.
    pub device: Option<PathBuf>,
    /// Switch the board's GPIOs to DPI on start and back to idle-low on stop.
    pub manage_pins: bool,
    /// SoC family; `None` reads `/proc/device-tree/model`.
    pub soc: Option<DpiSoc>,
    /// Longest wait for a page flip before reporting an error.
    pub flip_timeout: Duration,
}

impl DpiConfig {
    /// Defaults for `board`: auto-detect everything, manage pins.
    pub fn for_board(board: BoardKind) -> Self {
        DpiConfig {
            board,
            device: None,
            manage_pins: true,
            soc: None,
            flip_timeout: Duration::from_millis(500),
        }
    }
}

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

struct MappedBuffer {
    dumb: DumbBuffer,
    fb: framebuffer::Handle,
    ptr: *mut u8,
    len: usize,
    state: BufferState,
}

impl MappedBuffer {
    fn create(card: &Card, width: u32, height: u32) -> Result<MappedBuffer> {
        let dumb = card
            .create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, 32)
            .map_err(|e| OutputError::io("creating DRM dumb buffer", e))?;
        let fb = match card.add_framebuffer(&dumb, 24, 32) {
            Ok(fb) => fb,
            Err(e) => {
                let _ = card.destroy_dumb_buffer(dumb);
                return Err(OutputError::io("adding DRM framebuffer", e));
            }
        };
        let len = dumb.pitch() as usize * height as usize;
        let map = drm_ffi::mode::dumbbuffer::map(card.as_fd(), u32::from(dumb.handle()), 0, 0);
        let ptr = map.ok().and_then(|info| {
            let offset = libc::off_t::try_from(info.offset).ok()?;
            // SAFETY: mapping a DRM dumb buffer at the offset the kernel gave
            // us; checked for MAP_FAILED and unmapped in `destroy`.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    card.0.as_raw_fd(),
                    offset,
                )
            };
            (p != libc::MAP_FAILED).then_some(p.cast::<u8>())
        });
        let Some(ptr) = ptr else {
            let err = std::io::Error::last_os_error();
            let _ = card.destroy_framebuffer(fb);
            let _ = card.destroy_dumb_buffer(dumb);
            return Err(OutputError::io("mapping DRM dumb buffer", err));
        };
        Ok(MappedBuffer {
            dumb,
            fb,
            ptr,
            len,
            state: BufferState::new(),
        })
    }

    fn view(&mut self, width: u32, height: u32) -> Result<FrameBufferMut<'_>> {
        // SAFETY: ptr/len describe a live, exclusively owned mapping; the
        // returned borrow is tied to &mut self.
        let bytes = unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) };
        FrameBufferMut::from_bytes(
            bytes,
            width as usize,
            height as usize,
            self.dumb.pitch() as usize,
        )
    }

    fn destroy(self, card: &Card) {
        // SAFETY: ptr/len come from a successful mmap in `create`.
        unsafe {
            libc::munmap(self.ptr.cast(), self.len);
        }
        let _ = card.destroy_framebuffer(self.fb);
        let _ = card.destroy_dumb_buffer(self.dumb);
    }
}

struct Runtime {
    card: Card,
    device: PathBuf,
    crtc: crtc::Handle,
    connector: connector::Handle,
    saved_crtc: Option<crtc::Info>,
    buffers: Vec<MappedBuffer>,
    front: usize,
    pending: Option<usize>,
    encoder: WsEncoder,
    pins: Option<(PinMux, Vec<u8>)>,
}

// SAFETY: the raw pointers in `MappedBuffer` point into mappings owned by
// this runtime and are only dereferenced through `&mut self`.
unsafe impl Send for Runtime {}

impl Runtime {
    fn geometry(&self) -> DpiGeometry {
        *self.encoder.geometry()
    }

    /// Wait for the queued page flip (if any) to complete.
    fn wait_flip(&mut self, timeout: Duration) -> Result<Duration> {
        let Some(index) = self.pending else {
            return Ok(Duration::ZERO);
        };
        let started = Instant::now();
        loop {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(OutputError::Timeout(format!(
                    "a page flip on {} (is the DPI display enabled?)",
                    self.device.display()
                )));
            }
            let mut pfd = libc::pollfd {
                fd: self.card.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = libc::c_int::try_from(left.as_millis().max(1)).unwrap_or(libc::c_int::MAX);
            // SAFETY: one valid pollfd for the duration of the call.
            let n = unsafe { libc::poll(&mut pfd, 1, ms) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(OutputError::io("waiting for page flip", err));
            }
            if n == 0 {
                continue;
            }
            let events = self
                .card
                .receive_events()
                .map_err(|e| OutputError::io("reading DRM events", e))?;
            for event in events {
                if let Event::PageFlip(flip) = event {
                    if flip.crtc == self.crtc {
                        self.front = index;
                        self.pending = None;
                    }
                }
            }
            if self.pending.is_none() {
                return Ok(started.elapsed());
            }
        }
    }

    fn present(&mut self, frame: &OutputFrameRef<'_>, timeout: Duration) -> Result<(Duration, bool)> {
        let waited = self.wait_flip(timeout)?;
        let back = (self.front + 1) % self.buffers.len();
        let g = self.geometry();
        let (w, h) = (g.hactive(), g.vactive());
        let buffer = &mut self.buffers[back];
        let mut state = std::mem::take(&mut buffer.state);
        let result = buffer
            .view(w, h)
            .and_then(|mut fb| self.encoder.encode(frame, &mut fb, &mut state));
        buffer.state = state;
        let report = result?;
        let fb = self.buffers[back].fb;
        self.card
            .page_flip(self.crtc, fb, PageFlipFlags::EVENT, None)
            .map_err(|e| OutputError::io("queueing page flip", e))?;
        self.pending = Some(back);
        Ok((waited, report.truncated_outputs > 0))
    }
}

/// WS281x output through the DPI peripheral (see the module docs).
pub struct DpiOutput {
    config: DpiConfig,
    layout: OutputLayout,
    stats: OutputStats,
    rt: Option<Runtime>,
}

impl std::fmt::Debug for DpiOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DpiOutput")
            .field("config", &self.config)
            .field("running", &self.rt.is_some())
            .finish()
    }
}

impl DpiOutput {
    /// A DPI output for `config.board`; nothing is opened until [`PixelOutput::start`].
    pub fn new(config: DpiConfig) -> Self {
        let layout = OutputLayout::for_board(config.board);
        let mut stats = OutputStats::new("dpi");
        stats.outputs = layout.output_count();
        DpiOutput {
            config,
            layout,
            stats,
            rt: None,
        }
    }

    /// The geometry of the running display mode.
    pub fn geometry(&self) -> Option<DpiGeometry> {
        self.rt.as_ref().map(Runtime::geometry)
    }

    /// The DRM device in use.
    pub fn device(&self) -> Option<&Path> {
        self.rt.as_ref().map(|rt| rt.device.as_path())
    }

    fn open(&self) -> Result<Runtime> {
        if self.layout.output_count() == 0 {
            return Err(OutputError::Unsupported(format!(
                "{} has no pixel outputs",
                self.config.board.display_name()
            )));
        }
        let soc = match self.config.soc {
            Some(soc) => soc,
            None => detect_soc()?,
        };
        let (card, device, conn, mode) = find_dpi_connector(self.config.device.as_deref())?;
        let (hd, vd) = mode.size();
        let (hss, hse, ht) = mode.hsync();
        let (vss, vse, vt) = mode.vsync();
        let geometry = DpiGeometry::from_mode(
            mode.clock().saturating_mul(1000),
            u32::from(hd),
            u32::from(hss),
            u32::from(hse),
            u32::from(ht),
            u32::from(vd),
            u32::from(vss),
            u32::from(vse),
            u32::from(vt),
        )?;
        let encoder = WsEncoder::new(self.layout.clone(), geometry)?;

        // Mode setting needs DRM master. Failing here usually means a desktop
        // session owns the display; set_crtc below reports it precisely.
        if let Err(e) = card.acquire_master_lock() {
            tracing::debug!("DRM master lock not acquired on {}: {e}", device.display());
        }
        let crtc = pick_crtc(&card, &conn)?;
        let saved_crtc = card.get_crtc(crtc).ok();

        let mut buffers = Vec::with_capacity(2);
        for _ in 0..2 {
            match MappedBuffer::create(&card, geometry.hactive(), geometry.vactive()) {
                Ok(b) => buffers.push(b),
                Err(e) => {
                    for b in buffers {
                        b.destroy(&card);
                    }
                    return Err(e);
                }
            }
        }
        let mut rt = Runtime {
            card,
            device,
            crtc,
            connector: conn,
            saved_crtc,
            buffers,
            front: 0,
            pending: None,
            encoder,
            pins: None,
        };
        // Both buffers start as a valid idle waveform (all lines low).
        let empty = OutputFrameRef::default();
        for i in 0..rt.buffers.len() {
            let g = rt.geometry();
            let buffer = &mut rt.buffers[i];
            let mut state = BufferState::new();
            let result = buffer
                .view(g.hactive(), g.vactive())
                .and_then(|mut fb| rt.encoder.encode(&empty, &mut fb, &mut state));
            buffer.state = state;
            if let Err(e) = result {
                rt.teardown();
                return Err(e);
            }
        }
        if let Err(e) = rt
            .card
            .set_crtc(crtc, Some(rt.buffers[0].fb), (0, 0), &[conn], Some(mode))
        {
            rt.teardown();
            return Err(OutputError::io(
                "setting the DPI display mode (is another program, e.g. a desktop, using the display?)",
                e,
            ));
        }
        if self.config.manage_pins {
            let pins = self.layout.gpio_pins();
            let mux = match PinMux::detect(soc) {
                Ok(m) => m,
                Err(e) => {
                    rt.teardown();
                    return Err(e);
                }
            };
            if let Err(e) = mux.set_dpi(&pins) {
                let _ = mux.set_idle(&pins);
                rt.teardown();
                return Err(e);
            }
            rt.pins = Some((mux, pins));
        }
        tracing::info!(
            device = %rt.device.display(),
            pixels_per_output = geometry.pixels_per_output,
            refresh_hz = geometry.refresh_hz(),
            "DPI pixel output started"
        );
        Ok(rt)
    }
}

impl Runtime {
    /// Release pins, restore the previous display state and free buffers.
    fn teardown(mut self) {
        if let Some((mux, pins)) = self.pins.take() {
            if let Err(e) = mux.set_idle(&pins) {
                tracing::warn!("could not return pixel pins to idle: {e}");
            }
        }
        if let Some(saved) = self.saved_crtc.take() {
            if let (Some(fb), Some(mode)) = (saved.framebuffer(), saved.mode()) {
                // Best effort: give the display back to whoever had it (the
                // pins are already idle, so its content never reaches pixels).
                let _ = self.card.set_crtc(
                    self.crtc,
                    Some(fb),
                    saved.position(),
                    &[self.connector],
                    Some(mode),
                );
            }
        }
        for b in self.buffers.drain(..) {
            b.destroy(&self.card);
        }
        let _ = self.card.release_master_lock();
    }
}

impl PixelOutput for DpiOutput {
    fn start(&mut self) -> Result<()> {
        if self.rt.is_some() {
            return Ok(());
        }
        match self.open() {
            Ok(rt) => {
                let g = rt.geometry();
                self.stats.refresh_hz = Some(g.refresh_hz());
                self.stats.max_pixels_per_output = Some(g.pixels_per_output);
                self.stats.running = true;
                self.rt = Some(rt);
                Ok(())
            }
            Err(e) => {
                self.stats.record_error(e.to_string());
                Err(e)
            }
        }
    }

    fn write_frame(&mut self, frame: &OutputFrameRef<'_>) -> Result<()> {
        let Some(rt) = self.rt.as_mut() else {
            return Err(OutputError::NotRunning);
        };
        let started = Instant::now();
        match rt.present(frame, self.config.flip_timeout) {
            Ok((waited, truncated)) => {
                let waited_us = u64::try_from(waited.as_micros()).unwrap_or(u64::MAX);
                self.stats.last_wait_us = waited_us;
                self.stats
                    .record_encode(started.elapsed().saturating_sub(waited));
                self.stats.truncated_frames += u64::from(truncated);
                self.stats.frames += 1;
                Ok(())
            }
            Err(e) => {
                self.stats.record_error(e.to_string());
                Err(e)
            }
        }
    }

    fn stop(&mut self) {
        let Some(mut rt) = self.rt.take() else {
            return;
        };
        // Send black to every LED of every output, let it scan out completely,
        // then park the pins low.
        let g = rt.geometry();
        let black = OutputFrame::black(self.layout.output_count(), g.pixels_per_output as usize);
        let timeout = self.config.flip_timeout;
        let blanked = rt
            .present(&black.as_frame_ref(), timeout)
            .and_then(|_| rt.wait_flip(timeout));
        match blanked {
            Ok(_) => std::thread::sleep(Duration::from_nanos(g.frame_ns() as u64) + Duration::from_millis(2)),
            Err(e) => tracing::warn!("could not blank pixel outputs before stopping: {e}"),
        }
        rt.teardown();
        self.stats.running = false;
        tracing::info!("DPI pixel output stopped");
    }

    fn stats(&self) -> OutputStats {
        self.stats.clone()
    }
}

impl Drop for DpiOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Classify the running Pi from the device tree.
pub fn detect_soc() -> Result<DpiSoc> {
    let model = std::fs::read_to_string(DEVICE_TREE_MODEL).map_err(|e| {
        OutputError::io(
            format!("reading {DEVICE_TREE_MODEL} (is this a Raspberry Pi?)"),
            e,
        )
    })?;
    DpiSoc::from_model(&model).ok_or_else(|| {
        OutputError::Unsupported(format!(
            "`{}` is not a Raspberry Pi; DPI output needs one",
            model.trim_end_matches('\0').trim()
        ))
    })
}

/// DRM card nodes to scan, in order.
fn card_candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(p) = explicit {
        return vec![p.to_path_buf()];
    }
    let mut cards: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("card"))
                })
                .collect()
        })
        .unwrap_or_default();
    cards.sort();
    cards
}

fn find_dpi_connector(explicit: Option<&Path>) -> Result<(Card, PathBuf, connector::Handle, Mode)> {
    let candidates = card_candidates(explicit);
    if candidates.is_empty() {
        return Err(OutputError::DeviceNotFound(
            "no /dev/dri/card* devices; is the KMS driver (dtoverlay=vc4-kms-v3d) enabled?".into(),
        ));
    }
    let mut problems = Vec::new();
    for path in candidates {
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                problems.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let card = Card(file);
        let Ok(res) = card.resource_handles() else {
            continue; // render-only node (e.g. v3d)
        };
        for &handle in res.connectors() {
            let Ok(info) = card.get_connector(handle, false) else {
                continue;
            };
            if info.interface() != connector::Interface::DPI {
                continue;
            }
            let info = if info.modes().is_empty() {
                match card.get_connector(handle, true) {
                    Ok(i) => i,
                    Err(_) => info,
                }
            } else {
                info
            };
            let mode = info
                .modes()
                .iter()
                .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                .or_else(|| info.modes().first())
                .copied();
            match mode {
                Some(mode) => return Ok((card, path, handle, mode)),
                None => problems.push(format!("{}: DPI connector has no mode", path.display())),
            }
        }
    }
    let detail = if problems.is_empty() {
        String::new()
    } else {
        format!(" ({})", problems.join("; "))
    };
    Err(OutputError::DeviceNotFound(format!(
        "no DPI display connector found{detail}. Add the PixelPlus overlay to \
         /boot/firmware/config.txt (`pixelplus config-txt`) and reboot"
    )))
}

fn pick_crtc(card: &Card, conn: &connector::Handle) -> Result<crtc::Handle> {
    let res = card
        .resource_handles()
        .map_err(|e| OutputError::io("reading DRM resources", e))?;
    let info = card
        .get_connector(*conn, false)
        .map_err(|e| OutputError::io("reading DPI connector", e))?;
    let encoders = info
        .current_encoder()
        .into_iter()
        .chain(info.encoders().iter().copied());
    for enc in encoders {
        let Ok(e) = card.get_encoder(enc) else {
            continue;
        };
        if let Some(c) = e.crtc() {
            return Ok(c);
        }
        if let Some(&c) = res.filter_crtcs(e.possible_crtcs()).first() {
            return Ok(c);
        }
    }
    Err(OutputError::DeviceNotFound(
        "the DPI connector has no usable CRTC".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_device_is_a_clear_error() {
        let mut cfg = DpiConfig::for_board(BoardKind::Difftx);
        cfg.device = Some(PathBuf::from("/nonexistent/card0"));
        cfg.soc = Some(DpiSoc::Bcm2711);
        let mut out = DpiOutput::new(cfg);
        let err = out.start().unwrap_err();
        assert!(matches!(err, OutputError::DeviceNotFound(_)), "{err}");
        assert!(err.to_string().contains("config-txt"));
        assert_eq!(out.stats().errors, 1);
        let px = [0u8; 3];
        assert!(matches!(
            out.write_frame(&OutputFrameRef::new(vec![&px])),
            Err(OutputError::NotRunning)
        ));
        out.stop();
    }

    #[test]
    fn board_without_outputs_is_unsupported() {
        let mut out = DpiOutput::new(DpiConfig::for_board(BoardKind::BarePi));
        assert!(matches!(out.start(), Err(OutputError::Unsupported(_))));
    }
}
