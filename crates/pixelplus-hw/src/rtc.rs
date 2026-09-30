//! DS3231 battery-backed real-time clock (difftxlarge, I²C 0x68).
//!
//! **Normal operation** uses the kernel driver: the image's `config.txt` has
//! `dtoverlay=i2c-rtc,ds3231` (see `pixelplus config-txt --board difftxlarge`),
//! which binds `rtc-ds1307` to 0x68 and creates `/dev/rtcN`; systemd then sets
//! the system clock from it at boot and `hwclock`/`timedatectl` keep it in
//! sync. On a Pi 5 the SoC's own RTC is `rtc0` and the DS3231 becomes `rtc1`.
//!
//! This driver talks to the chip directly (it works whether or not the kernel
//! driver is bound) for `pixelplus detect`/`doctor` diagnostics and for
//! setting the clock on first boot. The DS3231 keeps UTC.

use crate::error::{HwError, Result};
use crate::i2c::I2cBus;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use serde::Serialize;

/// DS3231 I²C address.
pub const DS3231_ADDR: u8 = 0x68;

const REG_TIME: u8 = 0x00;
const REG_STATUS: u8 = 0x0F;
const REG_TEMP: u8 = 0x11;
const STATUS_OSF: u8 = 0x80;

/// A time read from the RTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtcTime {
    /// The stored time (UTC).
    pub utc: NaiveDateTime,
    /// The oscillator stopped since the time was last set (battery flat or
    /// never set): `utc` must not be trusted.
    pub oscillator_stopped: bool,
}

fn from_bcd(b: u8, max: u8, what: &str) -> Result<u8> {
    let (hi, lo) = (b >> 4, b & 0x0F);
    if hi > 9 || lo > 9 {
        return Err(HwError::InvalidData(format!("RTC {what} register 0x{b:02x} is not BCD")));
    }
    let v = hi * 10 + lo;
    if v > max {
        return Err(HwError::InvalidData(format!("RTC {what} {v} out of range")));
    }
    Ok(v)
}

fn to_bcd(v: u32) -> u8 {
    (((v / 10) << 4) | (v % 10)) as u8
}

/// Decode the seven time registers (0x00..=0x06).
pub fn decode_time(r: &[u8; 7]) -> Result<NaiveDateTime> {
    let sec = from_bcd(r[0] & 0x7F, 59, "seconds")?;
    let min = from_bcd(r[1] & 0x7F, 59, "minutes")?;
    let hour = if r[2] & 0x40 != 0 {
        // 12-hour mode: bit 5 = PM.
        let h12 = from_bcd(r[2] & 0x1F, 12, "hours")?;
        if h12 == 0 {
            return Err(HwError::InvalidData("RTC 12-hour clock reads hour 0".into()));
        }
        (h12 % 12) + if r[2] & 0x20 != 0 { 12 } else { 0 }
    } else {
        from_bcd(r[2] & 0x3F, 23, "hours")?
    };
    let day = from_bcd(r[4] & 0x3F, 31, "date")?;
    let month = from_bcd(r[5] & 0x1F, 12, "month")?;
    let century = if r[5] & 0x80 != 0 { 100 } else { 0 };
    let year = 2000 + century + i32::from(from_bcd(r[6], 99, "year")?);
    NaiveDate::from_ymd_opt(year, u32::from(month), u32::from(day))
        .and_then(|d| d.and_hms_opt(u32::from(hour), u32::from(min), u32::from(sec)))
        .ok_or_else(|| {
            HwError::InvalidData(format!(
                "RTC holds an impossible date {year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
            ))
        })
}

/// Encode a time into the seven time registers (24-hour mode).
pub fn encode_time(t: &NaiveDateTime) -> Result<[u8; 7]> {
    let year = t.year();
    if !(2000..=2199).contains(&year) {
        return Err(HwError::InvalidArgument(format!(
            "the DS3231 stores years 2000-2199, not {year}"
        )));
    }
    let y = (year - 2000) as u32;
    let century = if y >= 100 { 0x80 } else { 0 };
    Ok([
        to_bcd(t.second().min(59)),
        to_bcd(t.minute()),
        to_bcd(t.hour()),
        to_bcd(t.weekday().number_from_monday()),
        to_bcd(t.day()),
        to_bcd(t.month()) | century,
        to_bcd(y % 100),
    ])
}

/// A DS3231 on an [`I2cBus`].
pub struct Ds3231<'a> {
    bus: &'a mut dyn I2cBus,
    addr: u8,
}

impl<'a> Ds3231<'a> {
    /// The DS3231 at its fixed address 0x68.
    pub fn new(bus: &'a mut dyn I2cBus) -> Self {
        Ds3231 {
            bus,
            addr: DS3231_ADDR,
        }
    }

    /// Read the current time and the oscillator-stop flag.
    pub fn read_time(&mut self) -> Result<RtcTime> {
        let mut regs = [0u8; 7];
        self.bus.write_read(self.addr, &[REG_TIME], &mut regs)?;
        let mut status = [0u8; 1];
        self.bus.write_read(self.addr, &[REG_STATUS], &mut status)?;
        Ok(RtcTime {
            utc: decode_time(&regs)?,
            oscillator_stopped: status[0] & STATUS_OSF != 0,
        })
    }

    /// Set the time (UTC) and clear the oscillator-stop flag.
    pub fn set_time(&mut self, utc: &NaiveDateTime) -> Result<()> {
        let regs = encode_time(utc)?;
        let mut msg = [0u8; 8];
        msg[0] = REG_TIME;
        msg[1..].copy_from_slice(&regs);
        self.bus.write(self.addr, &msg)?;
        let mut status = [0u8; 1];
        self.bus.write_read(self.addr, &[REG_STATUS], &mut status)?;
        self.bus
            .write(self.addr, &[REG_STATUS, status[0] & !STATUS_OSF])
    }

    /// The DS3231's internal temperature sensor (0.25 °C steps).
    pub fn temperature(&mut self) -> Result<f64> {
        let mut t = [0u8; 2];
        self.bus.write_read(self.addr, &[REG_TEMP], &mut t)?;
        let raw = i16::from_be_bytes(t) >> 6;
        Ok(f64::from(raw) * 0.25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i2c::{MockByteRegisters, MockI2c};

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
    }

    #[test]
    fn encode_decode_round_trip() {
        for t in [dt(2026, 12, 24, 23, 59, 58), dt(2000, 1, 1, 0, 0, 0), dt(2150, 2, 28, 12, 30, 5)] {
            assert_eq!(decode_time(&encode_time(&t).unwrap()).unwrap(), t);
        }
        assert!(encode_time(&dt(1999, 1, 1, 0, 0, 0)).is_err());
    }

    #[test]
    fn twelve_hour_mode() {
        // 12:05:00 PM and 12:05:00 AM
        let pm = [0x00, 0x05, 0x40 | 0x20 | 0x12, 1, 0x01, 0x01, 0x26];
        assert_eq!(decode_time(&pm).unwrap(), dt(2026, 1, 1, 12, 5, 0));
        let am = [0x00, 0x05, 0x40 | 0x12, 1, 0x01, 0x01, 0x26];
        assert_eq!(decode_time(&am).unwrap(), dt(2026, 1, 1, 0, 5, 0));
        let pm7 = [0x00, 0x00, 0x40 | 0x20 | 0x07, 1, 0x01, 0x01, 0x26];
        assert_eq!(decode_time(&pm7).unwrap().hour(), 19);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_time(&[0xFF; 7]).is_err());
        assert!(decode_time(&[0, 0, 0, 1, 0x31, 0x02, 0x26]).is_err()); // 31 Feb
        assert!(decode_time(&[0, 0, 0x40, 1, 0x01, 0x01, 0x26]).is_err()); // 12h hour 0
    }

    #[test]
    fn device_round_trip() {
        let mut regs = MockByteRegisters::new(0x13, 1, 0);
        regs.mem[0x0F] = 0x88; // OSF set
        regs.mem[0x11] = 0x19; // 25.75 °C
        regs.mem[0x12] = 0xC0;
        let mut bus = MockI2c::new().with(DS3231_ADDR, regs);
        let mut rtc = Ds3231::new(&mut bus);
        let t = dt(2026, 11, 28, 17, 45, 0);
        rtc.set_time(&t).unwrap();
        let read = rtc.read_time().unwrap();
        assert_eq!(read.utc, t);
        assert!(!read.oscillator_stopped);
        assert_eq!(rtc.temperature().unwrap(), 25.75);
        let mut empty = MockI2c::new();
        assert!(Ds3231::new(&mut empty).read_time().is_err());
    }
}
