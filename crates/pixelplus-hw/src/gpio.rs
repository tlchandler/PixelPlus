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
    pressed: bool,
    last_change: Option<Duration>,
}

/// Lock-out debouncer: after an accepted change, further edges on the same
/// pin are ignored for `period`; an edge that does not change the settled
/// state is ignored.
#[derive(Debug, Clone)]
pub struct Debouncer {
    period: Duration,
    pins: BTreeMap<u8, PinState>,
}

impl Debouncer {
    /// A debouncer with the given lock-out period.
    pub fn new(period: Duration) -> Self {
        Debouncer {
            period,
            pins: BTreeMap::new(),
        }
    }

    /// Feed a raw edge; returns the event if it is a genuine change.
    pub fn feed(&mut self, gpio: u8, pressed: bool, at: Duration) -> Option<ButtonEvent> {
        let st = self.pins.entry(gpio).or_insert(PinState {
            pressed: false,
            last_change: None,
        });
        if st.pressed == pressed {
            return None;
        }
        if let Some(last) = st.last_change {
            if at.saturating_sub(last) < self.period {
                return None;
            }
        }
        st.pressed = pressed;
        st.last_change = Some(at);
        Some(ButtonEvent { gpio, pressed, at })
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
        while let Some((gpio, pressed, at)) = self.raw.pop_front() {
            if let Some(ev) = self.debouncer.feed(gpio, pressed, at) {
                return Ok(Some(ev));
            }
        }
        Ok(None)
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

    impl ButtonSource for GpioButtons {
        fn wait(&mut self, timeout: Duration) -> Result<Option<ButtonEvent>> {
            let deadline = Instant::now() + timeout;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let ready = self
                    .request
                    .wait_edge_event(left)
                    .map_err(|e| HwError::Unsupported(format!("waiting for GPIO edge: {e}")))?;
                if !ready {
                    return Ok(None);
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
                if left.is_zero() {
                    return Ok(None);
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
}
