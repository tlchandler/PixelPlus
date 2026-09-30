//! Board and SoC sensors: LM75B temperatures, the INA226 12 V monitor and the
//! Pi's own SoC temperature, as the `Sensor` objects of `GET /system/sensors`.
//!
//! For each chip PixelPlus first looks for a bound kernel **hwmon** driver
//! (`lm75`, `ina2xx`) and uses its sysfs files — reading the chip behind a
//! driver's back would, for the INA226, fight over the calibration register.
//! Without a driver it talks to the chip over **i2c-dev** and programs the
//! INA226 itself for the board's 10 mΩ shunt.

use crate::error::{HwError, Result};
use crate::i2c::I2cBus;
use pixelplus_core::model::BoardKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// LM75B addresses.
pub const LM75_ADDRS: [u8; 2] = [0x48, 0x49];
/// INA226 address on the difftxlarge.
pub const INA226_ADDR: u8 = 0x40;
/// difftxlarge input shunt.
pub const INA226_SHUNT_OHMS: f64 = 0.010;
/// INA226 current LSB chosen for an 8 A full scale (fuse is 5 A).
pub const INA226_CURRENT_LSB_A: f64 = 0.000_25;
/// Calibration value: 0.00512 / (current LSB × shunt) = 2048.
pub const INA226_CALIBRATION: u16 = 2048;
/// Configuration: 16-sample averaging, 1.1 ms conversions, continuous shunt + bus.
pub const INA226_CONFIG: u16 = 0x4527;

/// What a sensor measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorKind {
    /// Degrees Celsius.
    Temperature,
    /// Volts.
    Voltage,
    /// Amps.
    Current,
    /// Watts.
    Power,
}

/// One reading (`docs/ARCHITECTURE.md` §8 `GET /system/sensors`).
///
/// Threshold direction: for `voltage` sensors `warn`/`crit` are **minimums**
/// (alert when the value drops below); for every other kind they are
/// maximums. [`Sensor::status`] applies this rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensor {
    /// Stable id, e.g. `driverTemp`.
    pub id: String,
    /// Human label, e.g. `Driver temperature`.
    pub label: String,
    /// What is measured.
    pub kind: SensorKind,
    /// The reading.
    pub value: f64,
    /// Unit symbol (`°C`, `V`, `A`, `W`).
    pub unit: String,
    /// Warning threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<f64>,
    /// Critical threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit: Option<f64>,
}

/// Health of a reading relative to its thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorStatus {
    /// Within limits.
    Ok,
    /// Past the warning threshold.
    Warn,
    /// Past the critical threshold.
    Crit,
}

impl Sensor {
    /// Classify the reading (see the type docs for threshold direction).
    pub fn status(&self) -> SensorStatus {
        let past = |limit: Option<f64>| match (limit, self.kind) {
            (Some(l), SensorKind::Voltage) => self.value < l,
            (Some(l), _) => self.value > l,
            (None, _) => false,
        };
        if past(self.crit) {
            SensorStatus::Crit
        } else if past(self.warn) {
            SensorStatus::Warn
        } else {
            SensorStatus::Ok
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    SocThermal,
    Lm75(u8),
    InaBus,
    InaCurrent,
    InaPower,
}

#[derive(Debug, Clone, Copy)]
struct Spec {
    id: &'static str,
    label: &'static str,
    kind: SensorKind,
    source: Source,
    warn: Option<f64>,
    crit: Option<f64>,
}

const CPU: Spec = Spec {
    id: "cpuTemp",
    label: "CPU temperature",
    kind: SensorKind::Temperature,
    source: Source::SocThermal,
    warn: Some(75.0),
    crit: Some(82.0),
};

fn specs(board: BoardKind) -> Vec<Spec> {
    let temp = |id, label, addr, warn, crit| Spec {
        id,
        label,
        kind: SensorKind::Temperature,
        source: Source::Lm75(addr),
        warn: Some(warn),
        crit: Some(crit),
    };
    let mut v = vec![CPU];
    match board {
        BoardKind::Difftxlarge => {
            v.push(temp("driverTemp", "Driver temperature", 0x48, 60.0, 75.0));
            // The board's own OVER TEMP comparator trips at ~65.4 °C.
            v.push(temp(
                "powerTemp",
                "Power section temperature",
                0x49,
                55.0,
                65.0,
            ));
            v.push(Spec {
                id: "inputVoltage",
                label: "12 V input",
                kind: SensorKind::Voltage,
                source: Source::InaBus,
                warn: Some(11.0),
                crit: Some(10.5),
            });
            v.push(Spec {
                id: "inputCurrent",
                label: "Input current",
                kind: SensorKind::Current,
                source: Source::InaCurrent,
                warn: Some(4.0),
                crit: Some(4.75),
            });
            v.push(Spec {
                id: "inputPower",
                label: "Input power",
                kind: SensorKind::Power,
                source: Source::InaPower,
                warn: None,
                crit: None,
            });
        }
        BoardKind::Diffsmart => {
            // 0x48 sits in the hot corner between the FET bank and the main fuse.
            v.push(temp(
                "powerTemp",
                "Power section temperature",
                0x48,
                60.0,
                75.0,
            ));
            v.push(temp(
                "enclosureTemp",
                "Enclosure temperature",
                0x49,
                50.0,
                60.0,
            ));
        }
        _ => {}
    }
    v
}

fn unit(kind: SensorKind) -> &'static str {
    match kind {
        SensorKind::Temperature => "°C",
        SensorKind::Voltage => "V",
        SensorKind::Current => "A",
        SensorKind::Power => "W",
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Reads every sensor a board has. Missing or failing sensors are left out
/// (and logged at debug level) — reading never fails as a whole.
pub struct SensorHub {
    board: BoardKind,
    bus: Option<Box<dyn I2cBus>>,
    root: PathBuf,
    ina_configured: bool,
}

impl std::fmt::Debug for SensorHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SensorHub")
            .field("board", &self.board)
            .field("i2c", &self.bus.is_some())
            .field("root", &self.root)
            .finish()
    }
}

impl SensorHub {
    /// A hub for `board`; `bus` is the header I²C bus (None: sysfs only).
    pub fn new(board: BoardKind, bus: Option<Box<dyn I2cBus>>) -> Self {
        SensorHub {
            board,
            bus,
            root: PathBuf::from("/"),
            ina_configured: false,
        }
    }

    /// Read sysfs (`/sys/class/hwmon`, `/sys/class/thermal`) under `root` (tests, mocks).
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// The board.
    pub fn board(&self) -> BoardKind {
        self.board
    }

    /// Read all sensors now.
    pub fn read_all(&mut self) -> Vec<Sensor> {
        specs(self.board)
            .into_iter()
            .filter_map(|spec| match self.read(spec.source) {
                Ok(value) => Some(Sensor {
                    id: spec.id.to_string(),
                    label: spec.label.to_string(),
                    kind: spec.kind,
                    value: round3(value),
                    unit: unit(spec.kind).to_string(),
                    warn: spec.warn,
                    crit: spec.crit,
                }),
                Err(e) => {
                    tracing::debug!(sensor = spec.id, "sensor unavailable: {e}");
                    None
                }
            })
            .collect()
    }

    fn read(&mut self, source: Source) -> Result<f64> {
        match source {
            Source::SocThermal => {
                let p = self.root.join("sys/class/thermal/thermal_zone0/temp");
                Ok(read_number(&p)? / 1000.0)
            }
            Source::Lm75(addr) => {
                if let Some(dir) = hwmon_for(&self.root, 1, addr) {
                    return Ok(read_number(&dir.join("temp1_input"))? / 1000.0);
                }
                read_lm75(self.bus_mut()?, addr)
            }
            Source::InaBus | Source::InaCurrent | Source::InaPower => {
                if let Some(dir) = hwmon_for(&self.root, 1, INA226_ADDR) {
                    return match source {
                        Source::InaBus => Ok(read_number(&dir.join("in1_input"))? / 1000.0),
                        Source::InaCurrent => Ok(read_number(&dir.join("curr1_input"))? / 1000.0),
                        _ => Ok(read_number(&dir.join("power1_input"))? / 1_000_000.0),
                    };
                }
                if !self.ina_configured {
                    configure_ina226(self.bus_mut()?, INA226_ADDR)?;
                    self.ina_configured = true;
                }
                let bus = self.bus_mut()?;
                ensure_calibration(bus, INA226_ADDR)?;
                let r = read_ina226(bus, INA226_ADDR)?;
                Ok(match source {
                    Source::InaBus => r.bus_v,
                    Source::InaCurrent => r.current_a,
                    _ => r.bus_v * r.current_a.abs(),
                })
            }
        }
    }

    fn bus_mut(&mut self) -> Result<&mut dyn I2cBus> {
        match self.bus.as_mut() {
            Some(b) => Ok(b.as_mut()),
            None => Err(HwError::Unsupported("no I2C bus available".into())),
        }
    }
}

fn read_number(path: &Path) -> Result<f64> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| HwError::io(format!("reading {}", path.display()), e))?;
    s.trim().parse::<f64>().map_err(|_| {
        HwError::InvalidData(format!(
            "{}: `{}` is not a number",
            path.display(),
            s.trim()
        ))
    })
}

/// The hwmon directory of the kernel driver bound to `bus`-`addr`, if any.
pub fn hwmon_for(root: &Path, bus: u8, addr: u8) -> Option<PathBuf> {
    let want = format!("{bus}-{addr:04x}");
    let dir = std::fs::read_dir(root.join("sys/class/hwmon")).ok()?;
    dir.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
        std::fs::read_link(p.join("device"))
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy() == want))
            .unwrap_or(false)
    })
}

fn read_u16(bus: &mut dyn I2cBus, addr: u8, reg: u8) -> Result<u16> {
    let mut b = [0u8; 2];
    bus.write_read(addr, &[reg], &mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn write_u16(bus: &mut dyn I2cBus, addr: u8, reg: u8, value: u16) -> Result<()> {
    let [hi, lo] = value.to_be_bytes();
    bus.write(addr, &[reg, hi, lo])
}

/// Read an LM75/LM75B temperature register (0.125 °C resolution on LM75B).
pub fn read_lm75(bus: &mut dyn I2cBus, addr: u8) -> Result<f64> {
    let raw = read_u16(bus, addr, 0x00)? as i16;
    Ok(f64::from(raw >> 5) * 0.125)
}

/// Verify an INA226 is present and program configuration and calibration.
pub fn configure_ina226(bus: &mut dyn I2cBus, addr: u8) -> Result<()> {
    let maker = read_u16(bus, addr, 0xFE)?;
    let die = read_u16(bus, addr, 0xFF)?;
    if maker != 0x5449 || die & 0xFFF0 != 0x2260 {
        return Err(HwError::InvalidData(format!(
            "device at 0x{addr:02x} is not an INA226 (ids {maker:04x}/{die:04x})"
        )));
    }
    write_u16(bus, addr, 0x00, INA226_CONFIG)?;
    write_u16(bus, addr, 0x05, INA226_CALIBRATION)
}

/// One INA226 measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ina226Reading {
    /// Bus (12 V input) voltage, V (LSB 1.25 mV).
    pub bus_v: f64,
    /// Shunt current, A; positive when current flows IN+ → IN−.
    pub current_a: f64,
}

/// Read bus voltage and current.
///
/// The current is computed from the **shunt voltage register** (LSB 2.5 µV,
/// / 10 mΩ = 0.25 mA — bit-identical to the current register with
/// [`INA226_CALIBRATION`]) rather than from the current/power registers:
/// those are only recomputed at the end of the next averaged conversion
/// after the calibration register is written (datasheet §7.5), so the first
/// reading after configuration or a brown-out would report 0 A / 0 W.
pub fn read_ina226(bus: &mut dyn I2cBus, addr: u8) -> Result<Ina226Reading> {
    let shunt = read_u16(bus, addr, 0x01)? as i16;
    let vbus = read_u16(bus, addr, 0x02)?;
    Ok(Ina226Reading {
        // Bit 15 of the bus register is always 0 (unsigned, 0..40.96 V).
        bus_v: f64::from(vbus & 0x7FFF) * 0.001_25,
        current_a: f64::from(shunt) * 2.5e-6 / INA226_SHUNT_OHMS,
    })
}

fn ensure_calibration(bus: &mut dyn I2cBus, addr: u8) -> Result<()> {
    // A brown-out resets the chip to its power-on configuration
    // (calibration 0, 1-sample averaging); restore ours.
    if read_u16(bus, addr, 0x05)? != INA226_CALIBRATION
        || read_u16(bus, addr, 0x00)? != INA226_CONFIG
    {
        write_u16(bus, addr, 0x00, INA226_CONFIG)?;
        write_u16(bus, addr, 0x05, INA226_CALIBRATION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::{MockI2c, MockWordRegisters};

    fn fake_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pixelplus-sensors-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/class/thermal/thermal_zone0")).unwrap();
        std::fs::write(root.join("sys/class/thermal/thermal_zone0/temp"), "48312\n").unwrap();
        root
    }

    #[test]
    fn difftxlarge_over_i2c() {
        let root = fake_root("large");
        let bus = MockI2c::new()
            .with(0x40, MockWordRegisters::ina226(12.1, 1.5))
            .with(0x48, MockWordRegisters::lm75(35.5))
            .with(0x49, MockWordRegisters::lm75(-2.25));
        let mut hub = SensorHub::new(BoardKind::Difftxlarge, Some(Box::new(bus))).with_root(&root);
        let s = hub.read_all();
        let get = |id: &str| {
            s.iter()
                .find(|x| x.id == id)
                .unwrap_or_else(|| panic!("{id}: {s:?}"))
        };
        assert_eq!(get("cpuTemp").value, 48.312);
        assert_eq!(get("driverTemp").value, 35.5);
        assert_eq!(get("driverTemp").label, "Driver temperature");
        assert_eq!(get("powerTemp").value, -2.25);
        assert_eq!(get("inputVoltage").value, 12.1);
        assert_eq!(get("inputVoltage").label, "12 V input");
        assert_eq!(get("inputCurrent").value, 1.5);
        assert_eq!(get("inputPower").value, 18.15);
        assert_eq!(get("inputVoltage").unit, "V");
        // Second read reuses the configuration.
        assert_eq!(hub.read_all().len(), 6);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ina226_is_programmed() {
        let mut bus = MockI2c::new().with(0x40, MockWordRegisters::ina226(12.0, 0.0));
        configure_ina226(&mut bus, 0x40).unwrap();
        let dev = bus.device::<MockWordRegisters>(0x40).unwrap();
        assert_eq!(
            dev.written,
            vec![(0x00, INA226_CONFIG), (0x05, INA226_CALIBRATION)]
        );
        let mut wrong = MockI2c::new().with(0x40, MockWordRegisters::lm75(20.0));
        assert!(configure_ina226(&mut wrong, 0x40).is_err());
    }

    /// Right after the calibration register is written the INA226's current
    /// and power registers still hold values computed with the old (power-on
    /// 0) calibration until the next averaged conversion finishes (16 × 2.2 ms
    /// here). The first reading must not report 0 A / 0 W.
    #[test]
    fn first_reading_after_configuration_is_not_stale() {
        let mut dev = MockWordRegisters::ina226(12.0, 2.0);
        // Power-on state: calibration 0, so current and power read 0 and
        // stay 0 until a conversion completes after calibration.
        dev.regs.insert(0x04, 0);
        dev.regs.insert(0x03, 0);
        let bus = MockI2c::new().with(0x40, StaleIna(dev));
        let root = fake_root("stale");
        let mut hub = SensorHub::new(BoardKind::Difftxlarge, Some(Box::new(bus))).with_root(&root);
        let s = hub.read_all();
        let get = |id: &str| s.iter().find(|x| x.id == id).unwrap().value;
        assert_eq!(get("inputCurrent"), 2.0);
        assert_eq!(get("inputPower"), 24.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An INA226 whose current/power registers never update (conversion
    /// still in progress after configuration).
    struct StaleIna(MockWordRegisters);

    impl crate::i2c::MockDevice for StaleIna {
        fn write(&mut self, data: &[u8]) -> crate::Result<()> {
            let (cur, pow) = (self.0.regs[&0x04], self.0.regs[&0x03]);
            self.0.write(data)?;
            self.0.regs.insert(0x04, cur);
            self.0.regs.insert(0x03, pow);
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8]) -> crate::Result<()> {
            self.0.read(buf)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn negative_current_is_signed() {
        let mut bus = MockI2c::new().with(0x40, MockWordRegisters::ina226(12.0, -1.25));
        configure_ina226(&mut bus, 0x40).unwrap();
        let r = read_ina226(&mut bus, 0x40).unwrap();
        assert!((r.current_a + 1.25).abs() < 1e-9, "{r:?}");
        assert!((r.bus_v - 12.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn hwmon_preferred_when_bound() {
        let root = fake_root("hwmon");
        let hw = root.join("sys/class/hwmon/hwmon3");
        std::fs::create_dir_all(&hw).unwrap();
        std::os::unix::fs::symlink(
            "../../../devices/platform/soc/fe804000.i2c/i2c-1/1-0048",
            hw.join("device"),
        )
        .unwrap();
        std::fs::write(hw.join("temp1_input"), "41125\n").unwrap();
        let hw = root.join("sys/class/hwmon/hwmon4");
        std::fs::create_dir_all(&hw).unwrap();
        std::os::unix::fs::symlink("../../../devices/x/i2c-1/1-0049", hw.join("device")).unwrap();
        std::fs::write(hw.join("temp1_input"), "20000\n").unwrap();
        // No I2C bus at all: everything must come from sysfs.
        let mut hub = SensorHub::new(BoardKind::Diffsmart, None).with_root(&root);
        let s = hub.read_all();
        assert_eq!(s.len(), 3, "{s:?}");
        assert_eq!(s[1].id, "powerTemp");
        assert_eq!(s[1].value, 41.125);
        assert_eq!(s[2].label, "Enclosure temperature");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_sensors_are_skipped() {
        let root = std::env::temp_dir().join("pixelplus-sensors-none-at-all");
        let mut hub =
            SensorHub::new(BoardKind::Difftxlarge, Some(Box::new(MockI2c::new()))).with_root(root);
        assert!(hub.read_all().is_empty());
        assert_eq!(hub.board(), BoardKind::Difftxlarge);
    }

    #[test]
    fn status_direction_and_json() {
        let mut s = Sensor {
            id: "inputVoltage".into(),
            label: "12 V input".into(),
            kind: SensorKind::Voltage,
            value: 10.8,
            unit: "V".into(),
            warn: Some(11.0),
            crit: Some(10.5),
        };
        assert_eq!(s.status(), SensorStatus::Warn);
        s.value = 10.0;
        assert_eq!(s.status(), SensorStatus::Crit);
        s.kind = SensorKind::Temperature;
        s.value = 12.0;
        assert_eq!(s.status(), SensorStatus::Crit);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["kind"], "temperature");
        assert!(json.get("warn").is_some());
        s.warn = None;
        s.crit = None;
        assert_eq!(s.status(), SensorStatus::Ok);
        assert!(serde_json::to_value(&s).unwrap().get("warn").is_none());
    }
}
