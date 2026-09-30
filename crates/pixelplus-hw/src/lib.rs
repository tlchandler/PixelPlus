//! # PixelPlus hardware support
//!
//! Everything PixelPlus needs to know about the board it runs on, behind
//! small traits so it all runs against mocks on a development machine:
//!
//! | module | what |
//! |---|---|
//! | [`i2c`] | [`I2cBus`] trait, Linux `i2c-dev` implementation, simulated devices |
//! | [`eeprom`] | the `PPX1` board EEPROM format (read, write, validate, erase) |
//! | [`board`] | board detection (EEPROM + I²C probe) and Raspberry Pi model |
//! | [`sensors`] | LM75B, INA226 and SoC temperature as `Sensor` readings |
//! | [`rtc`] | DS3231 real-time clock |
//! | [`oled`] | SSD1306 status display with a built-in font |
//! | [`gpio`] | debounced button triggers on free GPIOs |
//! | [`mock`] | complete simulated boards for development |

#![warn(missing_docs)]

pub mod board;
pub mod eeprom;
mod error;
mod font;
pub mod gpio;
pub mod i2c;
pub mod mock;
pub mod oled;
pub mod rtc;
pub mod sensors;

pub use board::{BoardDetection, DetectionSource, PiFamily, PiInfo};
pub use eeprom::{EepromContents, EepromStore, Ppx1Record};
pub use error::{HwError, Result};
pub use i2c::{I2cBus, MockI2c};
pub use sensors::{Sensor, SensorHub, SensorKind, SensorStatus};

#[cfg(target_os = "linux")]
pub use i2c::LinuxI2c;
