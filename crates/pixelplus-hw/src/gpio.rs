//! Physical button triggers on spare GPIOs (`ShowSettings.triggers`, kind `gpio`).
//!
//! Buttons are read through the Linux GPIO character device (`gpiocdev`,
//! uAPI v2) with both-edge detection and kernel debouncing, plus a software
//! [`Debouncer`] so behaviour is identical on every kernel and in the mock.
//! Wiring: a normally-open button from the GPIO to GND; PixelPlus enables
//! the internal pull-up and reports a press on the falling edge.

use crate::error::{HwError, Result};
use pixelplus_core::model::BoardKind;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// Default debounce period.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(30);

/// GPIOs a board leaves free for triggers (BCM numbers, ascending).
///
/// GPIO0/1 (HAT ID EEPROM) and GPIO2/3 (I²C-1) are never offered; neither
/// are the board's pixel pins. On the difftxlarge every other header GPIO is
/// a pixel data or latch line, so only GPIO24 remains.
pub fn free_gpios(board: BoardKind) -> Vec<u8> {
    match board {
        BoardKind::Difftx | BoardKind::Diffsmart => (8..=27).collect(),
        BoardKind::Difftxlarge => vec![24],
        BoardKind::BarePi => (4..=27).collect(),
        BoardKind::Virtual => Vec::new(),
    }
}

/// Check `gpio` may be used as a trigger on `board`.
pub fn check_trigger_pin(board: BoardKind, gpio: u8) -> Result<()> {
    if free_gpios(board).contains(&gpio) {
        Ok(())
    } else {
        Err(HwError::InvalidArgument(format!(
            "GPIO{gpio} is not free on the {} (free: {})",
            board.display_name(),
            describe_pins(&free_gpios(board))
        )))
    }
}

fn describe_pins(pins: &[u8]) -> String {
    if pins.is_empty() {
        return "none".into();
    }
    pins.iter()
        .map(|p| format!("GPIO{p}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A debounced button transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonEvent {
    /// BCM GPIO number.
    pub gpio: u8,
    /// `true` when the button went down, `false` when released.
    pub pressed: bool,
    /// Kernel monotonic timestamp of the edge.
    pub at: Duration,
}

#[derive(Debug, Clone, Copy)]
struct PinState {
    /// Debounced (reported) level.
    pressed: bool,
    /// Level after the most recent raw edge.
    raw: bool,
    /// Time of the most recent raw edge.
    raw_at: Duration,
    /// Time of the last reported change.
    last_change: Option<Duration>,
}

impl PinState {
    fn lockout_ends(&self, period: Duration) -> Duration {
        self.last_change
            .map_or(Duration::ZERO, |t| t.saturating_add(period))
    }
}

/// Lock-out debouncer: after an accepted change, further edges on the same
/// pin are ignored for `period`; an edge that does not change the settled
/// state is ignored.
///
/// An edge that arrives inside the lock-out is not forgotten: if the line is
/// still at that level when the lock-out expires, the change is reported
/// then (by [`Debouncer::poll`], or by the next [`Debouncer::feed`]).
/// Otherwise a quick tap (release within `period` of the press) would leave
/// the debouncer believing the button is still held, and the next press
/// would be swallowed.
#[derive(Debug, Clone)]
pub struct Debouncer {
    period: Duration,
    pins: BTreeMap<u8, PinState>,
    ready: VecDeque<ButtonEvent>,
}

impl Debouncer {
    /// A debouncer with the given lock-out period.
    pub fn new(period: Duration) -> Self {
        Debouncer {
            period,
            pins: BTreeMap::new(),
            ready: VecDeque::new(),
        }
    }

    /// Report a pending level of `gpio` that has been stable until `now`
    /// and whose lock-out has expired.
    fn settle(&mut self, gpio: u8, now: Duration) {
        let period = self.period;
        if let Some(st) = self.pins.get_mut(&gpio) {
            if st.raw != st.pressed && now >= st.lockout_ends(period) {
                st.pressed = st.raw;
                st.last_change = Some(st.raw_at);
                self.ready.push_back(ButtonEvent {
                    gpio,
                    pressed: st.raw,
                    at: st.raw_at,
                });
            }
        }
    }

    /// Feed a raw edge (`at` on the same clock as [`Debouncer::poll`]);
    /// returns the next debounced event, if any. More than one event can
    /// become ready at once: fetch the rest with [`Debouncer::pop`].
    pub fn feed(&mut self, gpio: u8, pressed: bool, at: Duration) -> Option<ButtonEvent> {
        self.settle(gpio, at);
        let period = self.period;
        let st = self.pins.entry(gpio).or_insert(PinState {
            pressed: false,
            raw: false,
            raw_at: Duration::ZERO,
            last_change: None,
        });
        st.raw = pressed;
        st.raw_at = at;
        if st.pressed != pressed && at >= st.lockout_ends(period) {
            st.pressed = pressed;
            st.last_change = Some(at);
            self.ready.push_back(ButtonEvent { gpio, pressed, at });
        }
        self.ready.pop_front()
    }

    /// Report changes whose lock-out has expired by `now`.
    pub fn poll(&mut self, now: Duration) -> Option<ButtonEvent> {
        let pins: Vec<u8> = self.pins.keys().copied().collect();
        for gpio in pins {
            self.settle(gpio, now);
        }
        self.ready.pop_front()
    }

    /// An event that is ready but was not returned yet.
    pub fn pop(&mut self) -> Option<ButtonEvent> {
        self.ready.pop_front()
    }

    /// When the earliest pending change can be reported (call
    /// [`Debouncer::poll`] then); `None` if nothing is pending.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.pins
            .values()
            .filter(|st| st.raw != st.pressed)
            .map(|st| st.lockout_ends(self.period))
            .min()
    }
}

/// A source of debounced button events.
pub trait ButtonSource: Send {
    /// Wait up to `timeout` for the next event.
    fn wait(&mut self, timeout: Duration) -> Result<Option<ButtonEvent>>;
}

/// Scripted buttons for tests and non-Pi development.
#[derive(Debug, Clone)]
pub struct MockButtons {
    raw: VecDeque<(u8, bool, Duration)>,
    debouncer: Debouncer,
}

impl MockButtons {
    /// No pending edges.
    pub fn new(debounce: Duration) -> Self {
        MockButtons {
            raw: VecDeque::new(),
            debouncer: Debouncer::new(debounce),
        }
    }

    /// Queue a raw edge (level after the edge, as "pressed").
    pub fn push_edge(&mut self, gpio: u8, pressed: bool, at: Duration) {
        self.raw.push_back((gpio, pressed, at));
    }
}

impl ButtonSource for MockButtons {
    fn wait(&mut self, _timeout: Duration) -> Result<Option<ButtonEvent>> {
        if let Some(ev) = self.debouncer.pop() {
            return Ok(Some(ev));
        }
        while let Some((gpio, pressed, at)) = self.raw.pop_front() {
            if let Some(ev) = self.debouncer.feed(gpio, pressed, at) {
                return Ok(Some(ev));
            }
        }
        // Scripted time runs on past the last edge: flush pending changes.
        Ok(self.debouncer.poll(Duration::MAX))
    }
}

#[cfg(target_os = "linux")]
pub use linux::{find_header_chip, GpioButtons};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use gpiocdev::line::{Bias, EdgeDetection, EdgeKind};
    use std::path::PathBuf;
    use std::time::Instant;

    /// The gpiochip carrying the 40-pin header GPIOs (`pinctrl-bcm2835`,
    /// `pinctrl-bcm2711` or `pinctrl-rp1`).
    pub fn find_header_chip() -> Result<PathBuf> {
        let chips = gpiocdev::chip::chips()
            .map_err(|e| HwError::NotFound(format!("GPIO character devices: {e}")))?;
        chips
            .into_iter()
            .find(|p| {
                gpiocdev::Chip::from_path(p)
                    .and_then(|c| c.info())
                    .map(|i| i.label.starts_with("pinctrl-") && i.num_lines >= 28)
                    .unwrap_or(false)
            })
            .ok_or_else(|| HwError::NotFound("no Raspberry Pi header gpiochip".into()))
    }

    /// Buttons on real GPIOs.
    pub struct GpioButtons {
        request: gpiocdev::Request,
        debouncer: Debouncer,
    }

    impl std::fmt::Debug for GpioButtons {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("GpioButtons")
                .field("chip", &self.request.chip_path())
                .finish()
        }
    }

    impl GpioButtons {
        /// Watch `pins` (validated against the board's free GPIOs), with
        /// pull-ups and press = falling edge.
        pub fn open(board: BoardKind, pins: &[u8], debounce: Duration) -> Result<GpioButtons> {
            if pins.is_empty() {
                return Err(HwError::InvalidArgument("no trigger GPIOs given".into()));
            }
            for &p in pins {
                check_trigger_pin(board, p)?;
            }
            let chip = find_header_chip()?;
            let offsets: Vec<u32> = pins.iter().map(|&p| u32::from(p)).collect();
            let request = gpiocdev::Request::builder()
                .on_chip(&chip)
                .with_consumer("pixelplus-trigger")
                .with_lines(&offsets)
                .as_input()
                .with_bias(Bias::PullUp)
                .with_edge_detection(EdgeDetection::BothEdges)
                .with_debounce_period(debounce)
                .request()
                .map_err(|e| HwError::Unsupported(format!("requesting GPIOs {pins:?}: {e}")))?;
            Ok(GpioButtons {
                request,
                debouncer: Debouncer::new(debounce),
            })
        }
    }

    /// `CLOCK_MONOTONIC`, the clock of gpiocdev v2 edge event timestamps.
    fn monotonic_now() -> Duration {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes into the valid timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        Duration::new(
            u64::try_from(ts.tv_sec).unwrap_or(0),
            u32::try_from(ts.tv_nsec).unwrap_or(0),
        )
    }

    impl ButtonSource for GpioButtons {
        fn wait(&mut self, timeout: Duration) -> Result<Option<ButtonEvent>> {
            let deadline = Instant::now() + timeout;
            loop {
                if let Some(ev) = self.debouncer.pop() {
                    return Ok(Some(ev));
                }
                if let Some(ev) = self.debouncer.poll(monotonic_now()) {
                    return Ok(Some(ev));
                }
                let mut left = deadline.saturating_duration_since(Instant::now());
                // Wake up when a pending change's lock-out expires.
                if let Some(due) = self.debouncer.next_deadline() {
                    left = left.min(due.saturating_sub(monotonic_now()));
                }
                let ready = self
                    .request
                    .wait_edge_event(left)
                    .map_err(|e| HwError::Unsupported(format!("waiting for GPIO edge: {e}")))?;
                if !ready {
                    if Instant::now() >= deadline {
                        return Ok(self.debouncer.poll(monotonic_now()));
                    }
                    continue;
                }
                let ev = self
                    .request
                    .read_edge_event()
                    .map_err(|e| HwError::Unsupported(format!("reading GPIO edge: {e}")))?;
                let Ok(gpio) = u8::try_from(ev.offset) else {
                    continue;
                };
                let pressed = ev.kind == EdgeKind::Falling;
                if let Some(event) =
                    self.debouncer
                        .feed(gpio, pressed, Duration::from_nanos(ev.timestamp_ns))
                {
                    return Ok(Some(event));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_pins_per_board() {
        assert_eq!(free_gpios(BoardKind::Difftxlarge), vec![24]);
        let small = free_gpios(BoardKind::Difftx);
        assert!(!small.iter().any(|p| (0..=7).contains(p)));
        assert!(small.contains(&17) && small.contains(&27));
        assert!(free_gpios(BoardKind::Virtual).is_empty());
        assert!(check_trigger_pin(BoardKind::Difftxlarge, 24).is_ok());
        let err = check_trigger_pin(BoardKind::Difftxlarge, 17).unwrap_err();
        assert!(err.to_string().contains("GPIO24"));
        assert!(check_trigger_pin(BoardKind::Diffsmart, 5).is_err());
        assert!(check_trigger_pin(BoardKind::Virtual, 5)
            .unwrap_err()
            .to_string()
            .contains("none"));
    }

    #[test]
    fn debouncing() {
        let ms = Duration::from_millis;
        let mut b = MockButtons::new(ms(30));
        // Bouncy press: down, up, down within 5 ms, then a clean release.
        b.push_edge(24, true, ms(100));
        b.push_edge(24, false, ms(102));
        b.push_edge(24, true, ms(104));
        b.push_edge(24, false, ms(300));
        // Spurious "release" with no press.
        b.push_edge(17, false, ms(310));
        let first = b.wait(ms(0)).unwrap().unwrap();
        assert_eq!((first.gpio, first.pressed, first.at), (24, true, ms(100)));
        let second = b.wait(ms(0)).unwrap().unwrap();
        assert_eq!((second.pressed, second.at), (false, ms(300)));
        assert!(b.wait(ms(0)).unwrap().is_none());
    }

    /// A quick tap: the release edge arrives inside the lock-out after the
    /// press. It must still be reported once the lock-out expires, and the
    /// next press must not be lost.
    #[test]
    fn quick_tap_is_not_lost() {
        let ms = Duration::from_millis;
        let mut d = Debouncer::new(ms(30));
        assert!(d.feed(24, true, ms(100)).is_some());
        assert_eq!(d.feed(24, false, ms(110)), None, "inside the lock-out");
        assert_eq!(d.next_deadline(), Some(ms(130)));
        assert_eq!(d.poll(ms(120)), None);
        let release = d.poll(ms(130)).expect("release reported after lock-out");
        assert_eq!((release.pressed, release.at), (false, ms(110)));
        assert_eq!(d.next_deadline(), None);
        // Next press is a genuine change.
        let press = d.feed(24, true, ms(1000)).unwrap();
        assert!(press.pressed);

        // Same, but nobody polled in between: the next edge flushes the
        // pending release first, then reports the press.
        let mut b = MockButtons::new(ms(30));
        b.push_edge(24, true, ms(100));
        b.push_edge(24, false, ms(110));
        b.push_edge(24, true, ms(1000));
        let got: Vec<(bool, Duration)> = std::iter::from_fn(|| b.wait(ms(0)).unwrap())
            .map(|e| (e.pressed, e.at))
            .collect();
        assert_eq!(
            got,
            vec![(true, ms(100)), (false, ms(110)), (true, ms(1000))]
        );
    }
}
