//! `pixelplus detect`: board, Pi and sensors at a glance.

use crate::hwctx::HwContext;
use crate::style::{self, paint, Level};
use anyhow::Result;
use pixelplus_hw::board::DetectionSource;
use pixelplus_hw::eeprom;
use pixelplus_hw::sensors::{Sensor, SensorHub, SensorStatus};

/// What lives at each known I²C address.
pub fn device_name(addr: u8) -> &'static str {
    match addr {
        0x3C => "SSD1306 OLED",
        0x40 => "INA226 power monitor",
        0x48 | 0x49 => "LM75B temperature",
        0x50 => "AT24C256 EEPROM",
        0x68 => "DS3231 RTC",
        _ => "device",
    }
}

/// Map a sensor status to a CLI level.
pub fn level(status: SensorStatus) -> Level {
    match status {
        SensorStatus::Ok => Level::Ok,
        SensorStatus::Warn => Level::Warn,
        SensorStatus::Crit => Level::Fail,
    }
}

/// Read every sensor for the detected (or suggested) board.
pub fn read_sensors(ctx: &HwContext, board: pixelplus_core::model::BoardKind) -> Vec<Sensor> {
    let bus = ctx.bus().ok();
    SensorHub::new(board, bus).read_all()
}

/// `detect --json` output. image/firstboot/firstboot.py reads
/// `board.board` (board id or null) and `board.rev`; keep that shape.
pub fn json_report(
    simulated: bool,
    pi: &Option<pixelplus_hw::board::PiInfo>,
    detection: &pixelplus_hw::board::BoardDetection,
    sensors: &[Sensor],
) -> serde_json::Value {
    serde_json::json!({
        "simulated": simulated,
        "pi": pi,
        "board": detection,
        "sensors": sensors,
    })
}

/// Run `detect`.
pub fn run(ctx: &HwContext, json: bool) -> Result<()> {
    let pi = ctx.pi_info();
    let detection = ctx.detect()?;
    let board_for_sensors = detection
        .board
        .or(detection.suggested)
        .unwrap_or(pixelplus_core::model::BoardKind::BarePi);
    let sensors = read_sensors(ctx, board_for_sensors);

    if json {
        let out = json_report(ctx.is_simulated(), &pi, &detection, &sensors);
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if ctx.is_simulated() {
        anstream::println!("{}", paint(style::DIM, "(simulated hardware)"));
    }
    style::heading("System");
    match &pi {
        Some(p) => style::kv("Raspberry Pi", &p.model, 12),
        None => style::kv("Raspberry Pi", paint(style::WARN, "not a Raspberry Pi"), 12),
    }
    let board_line = match (detection.board, detection.source) {
        (Some(b), src) => {
            let rev = detection
                .rev
                .as_deref()
                .map(|r| format!(" rev {r}"))
                .unwrap_or_default();
            let serial = detection
                .serial
                .as_deref()
                .map(|s| format!(", serial {s}"))
                .unwrap_or_default();
            let from = match src {
                DetectionSource::Eeprom => "EEPROM",
                DetectionSource::FppEeprom => "FPP cape EEPROM",
                DetectionSource::None => "?",
            };
            format!(
                "{}{rev}{serial} {}",
                b.display_name(),
                paint(style::DIM, format!("[{from}]"))
            )
        }
        (None, _) => match detection.suggested {
            Some(s) => format!(
                "{} {}",
                paint(style::WARN, "unidentified"),
                paint(
                    style::DIM,
                    format!(
                        "(looks like {}; run `pixelplus eeprom write --board {} --rev <rev>`)",
                        s.display_name(),
                        eeprom::board_id(s)
                    )
                )
            ),
            None => paint(style::WARN, "none detected").to_string(),
        },
    };
    style::kv("Board", board_line, 12);
    let eeprom_state = detection
        .eeprom
        .as_ref()
        .map(|c| c.state_name())
        .unwrap_or("unreadable");
    style::kv("EEPROM", eeprom_state, 12);
    let devices = if detection.i2c_devices.is_empty() {
        "none".to_string()
    } else {
        detection
            .i2c_devices
            .iter()
            .map(|a| format!("0x{a:02x} {}", device_name(*a)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    style::kv("I2C devices", devices, 12);

    anstream::println!();
    style::heading("Sensors");
    if sensors.is_empty() {
        anstream::println!("  {}", paint(style::DIM, "no sensors readable"));
    }
    for s in &sensors {
        anstream::println!(
            "  {} {:<26} {:>9} {}",
            level(s.status()).badge(),
            s.label,
            format!("{:.2}", s.value),
            s.unit
        );
    }
    if !detection.warnings.is_empty() {
        anstream::println!();
        style::heading("Warnings");
        for w in &detection.warnings {
            anstream::println!("  {} {w}", Level::Warn.badge());
        }
    }
    Ok(())
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use pixelplus_core::model::BoardKind;

    /// The shape image/firstboot/firstboot.py (detect_board) depends on.
    #[test]
    fn json_matches_firstboot() {
        let ctx = HwContext {
            simulate: Some(BoardKind::Difftxlarge),
        };
        let det = ctx.detect().unwrap();
        let v = json_report(true, &ctx.pi_info(), &det, &[]);
        assert_eq!(v["board"]["board"], "difftxlarge");
        assert!(v["board"]["rev"].is_string());
        // Blank EEPROM: board.board is null (firstboot then leaves the boot config alone).
        let blank = pixelplus_hw::board::classify(None, &[]);
        let v = json_report(false, &None, &blank, &[]);
        assert!(v["board"].is_object());
        assert!(v["board"]["board"].is_null());
    }
}
