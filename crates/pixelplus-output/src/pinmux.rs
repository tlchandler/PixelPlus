//! Switching header GPIOs between "DPI output" and "idle low" (Linux only).
//!
//! The `pixelplus-dpi` overlays claim no pins, so the DPI signal reaches the
//! header only while PixelPlus has muxed exactly the board's pins to it. This
//! keeps boot-time console garbage off the pixels, leaves I²C (GPIO2/3) and
//! unused GPIOs alone, and makes "stop output" a hard electrical guarantee.
//!
//! Two mechanisms:
//!
//! 1. **`pinctrl`** (package `raspi-utils`, preinstalled on Raspberry Pi OS
//!    Bookworm/Trixie). Works on every model including the Pi 5's RP1.
//! 2. **`/dev/gpiomem`** register access for BCM283x/BCM2711 when `pinctrl`
//!    is missing (BCM2835 ARM Peripherals §6: GPFSELn, GPCLR0, and GPPUD /
//!    GPPUDCLK0 or, on BCM2711, GPIO_PUP_PDN_CNTRL_REGn).

use crate::error::{OutputError, Result};
use crate::pi_config::{gpio_ranges, DpiSoc};
use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// How pin functions are changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinMuxMethod {
    /// The Raspberry Pi `pinctrl` utility at this path.
    Pinctrl(PathBuf),
    /// Direct register access through `/dev/gpiomem` (BCM283x / BCM2711 only).
    GpioMem,
}

/// Switches a set of GPIOs between DPI function and driven-low GPIO.
#[derive(Debug, Clone)]
pub struct PinMux {
    soc: DpiSoc,
    method: PinMuxMethod,
}

fn find_pinctrl() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join("pinctrl"))
            .find(|p| p.is_file())
    });
    from_path.or_else(|| {
        ["/usr/bin/pinctrl", "/usr/sbin/pinctrl", "/usr/local/bin/pinctrl"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

impl PinMux {
    /// Pick the best available method for `soc`.
    pub fn detect(soc: DpiSoc) -> Result<PinMux> {
        if let Some(path) = find_pinctrl() {
            return Ok(PinMux {
                soc,
                method: PinMuxMethod::Pinctrl(path),
            });
        }
        if soc != DpiSoc::Bcm2712 && Path::new("/dev/gpiomem").exists() {
            return Ok(PinMux {
                soc,
                method: PinMuxMethod::GpioMem,
            });
        }
        Err(OutputError::PinMux(
            "neither `pinctrl` (apt install raspi-utils) nor /dev/gpiomem is available".into(),
        ))
    }

    /// Use a specific method.
    pub fn with_method(soc: DpiSoc, method: PinMuxMethod) -> PinMux {
        PinMux { soc, method }
    }

    /// The method in use.
    pub fn method(&self) -> &PinMuxMethod {
        &self.method
    }

    /// Route `pins` to the DPI block (with pull-downs enabled).
    pub fn set_dpi(&self, pins: &[u8]) -> Result<()> {
        check_pins(pins)?;
        match &self.method {
            PinMuxMethod::Pinctrl(path) => run_pinctrl(path, pins, &[self.soc.dpi_function(), "pd"]),
            PinMuxMethod::GpioMem => GpioMem::open()?.set_dpi(self.soc, pins),
        }
    }

    /// Make `pins` GPIO outputs driven low (WS281x idle), pull-downs enabled.
    pub fn set_idle(&self, pins: &[u8]) -> Result<()> {
        check_pins(pins)?;
        match &self.method {
            PinMuxMethod::Pinctrl(path) => run_pinctrl(path, pins, &["op", "dl", "pd"]),
            PinMuxMethod::GpioMem => GpioMem::open()?.set_idle(self.soc, pins),
        }
    }
}

fn check_pins(pins: &[u8]) -> Result<()> {
    match pins.iter().find(|&&p| !(4..=27).contains(&p)) {
        Some(p) => Err(OutputError::PinMux(format!(
            "GPIO{p} is not a DPI data pin (GPIO4-27); refusing to touch it"
        ))),
        None => Ok(()),
    }
}

fn run_pinctrl(program: &Path, pins: &[u8], options: &[&str]) -> Result<()> {
    // One invocation per contiguous range keeps the syntax portable across
    // pinctrl versions.
    for range in gpio_ranges(pins).split(',').filter(|r| !r.is_empty()) {
        let output = Command::new(program)
            .arg("set")
            .arg(range)
            .args(options)
            .output()
            .map_err(|e| OutputError::io(format!("running {}", program.display()), e))?;
        if !output.status.success() {
            return Err(OutputError::PinMux(format!(
                "`pinctrl set {range} {}` failed: {}",
                options.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}

/// A mapping of the BCM283x/BCM2711 GPIO register block.
struct GpioMem {
    base: *mut u32,
    len: usize,
}

const GPFSEL0: usize = 0x00;
const GPCLR0: usize = 0x28;
const GPPUD: usize = 0x94;
const GPPUDCLK0: usize = 0x98;
const GPIO_PUP_PDN_CNTRL_REG0: usize = 0xE4;
const FSEL_OUTPUT: u32 = 0b001;
const FSEL_ALT2: u32 = 0b110;

impl GpioMem {
    fn open() -> Result<GpioMem> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/gpiomem")
            .map_err(|e| OutputError::io("opening /dev/gpiomem", e))?;
        let len = 4096;
        // SAFETY: mapping a device file; the result is checked before use and
        // unmapped in Drop. The fd may be closed after mmap.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(OutputError::io(
                "mapping /dev/gpiomem",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(GpioMem {
            base: ptr.cast(),
            len,
        })
    }

    fn read(&self, offset: usize) -> u32 {
        debug_assert!(offset + 4 <= self.len);
        // SAFETY: offset is a fixed register offset inside the 4 KiB mapping.
        unsafe { self.base.add(offset / 4).read_volatile() }
    }

    fn write(&self, offset: usize, value: u32) {
        debug_assert!(offset + 4 <= self.len);
        // SAFETY: as in `read`.
        unsafe { self.base.add(offset / 4).write_volatile(value) }
    }

    fn set_function(&self, pin: u8, function: u32) {
        let reg = GPFSEL0 + usize::from(pin / 10) * 4;
        let shift = u32::from(pin % 10) * 3;
        let v = (self.read(reg) & !(0b111 << shift)) | (function << shift);
        self.write(reg, v);
    }

    fn pull_down(&self, soc: DpiSoc, pins: &[u8]) {
        match soc {
            DpiSoc::Bcm2711 => {
                for &pin in pins {
                    let reg = GPIO_PUP_PDN_CNTRL_REG0 + usize::from(pin / 16) * 4;
                    let shift = u32::from(pin % 16) * 2;
                    let v = (self.read(reg) & !(0b11 << shift)) | (0b10 << shift);
                    self.write(reg, v);
                }
            }
            _ => {
                let mask = pins.iter().fold(0u32, |m, &p| m | (1 << p));
                // Datasheet sequence: set control, wait 150 cycles, clock the
                // pins, wait 150 cycles, then remove both.
                self.write(GPPUD, 0b01);
                std::thread::sleep(Duration::from_micros(10));
                self.write(GPPUDCLK0, mask);
                std::thread::sleep(Duration::from_micros(10));
                self.write(GPPUD, 0);
                self.write(GPPUDCLK0, 0);
            }
        }
    }

    fn set_dpi(&self, soc: DpiSoc, pins: &[u8]) -> Result<()> {
        if soc == DpiSoc::Bcm2712 {
            return Err(OutputError::PinMux(
                "the Pi 5 needs `pinctrl` (apt install raspi-utils)".into(),
            ));
        }
        self.pull_down(soc, pins);
        for &pin in pins {
            self.set_function(pin, FSEL_ALT2);
        }
        Ok(())
    }

    fn set_idle(&self, soc: DpiSoc, pins: &[u8]) -> Result<()> {
        if soc == DpiSoc::Bcm2712 {
            return Err(OutputError::PinMux(
                "the Pi 5 needs `pinctrl` (apt install raspi-utils)".into(),
            ));
        }
        let mask = pins.iter().fold(0u32, |m, &p| m | (1 << p));
        self.write(GPCLR0, mask);
        for &pin in pins {
            self.set_function(pin, FSEL_OUTPUT);
        }
        self.pull_down(soc, pins);
        Ok(())
    }
}

impl Drop for GpioMem {
    fn drop(&mut self) {
        // SAFETY: base/len come from a successful mmap.
        unsafe {
            libc::munmap(self.base.cast(), self.len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_dpi_pins() {
        let mux = PinMux::with_method(DpiSoc::Bcm2711, PinMuxMethod::Pinctrl("/nonexistent".into()));
        assert!(matches!(mux.set_dpi(&[2, 3]), Err(OutputError::PinMux(_))));
        assert!(matches!(mux.set_idle(&[28]), Err(OutputError::PinMux(_))));
    }

    #[test]
    fn missing_program_is_an_error_not_a_panic() {
        let mux = PinMux::with_method(DpiSoc::Bcm2711, PinMuxMethod::Pinctrl("/nonexistent/pinctrl".into()));
        assert!(mux.set_idle(&[4, 5]).is_err());
    }

    #[test]
    fn fake_pinctrl_receives_ranges() {
        let dir = std::env::temp_dir().join(format!("pixelplus-pinctrl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let script = dir.join("pinctrl");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A concurrently forking test thread can briefly hold the script open
        // for writing (ETXTBSY); wait until it is executable.
        for _ in 0..50 {
            if Command::new(&script).arg("warmup").status().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(&log, "").unwrap();
        let mux = PinMux::with_method(DpiSoc::Bcm2712, PinMuxMethod::Pinctrl(script));
        let pins: Vec<u8> = (4..=23).chain(25..=27).collect();
        mux.set_dpi(&pins).unwrap();
        mux.set_idle(&[4, 5, 6, 7]).unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            logged,
            "set 4-23 a1 pd\nset 25-27 a1 pd\nset 4-7 op dl pd\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
