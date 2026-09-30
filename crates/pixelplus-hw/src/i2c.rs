//! I²C bus access: the Linux `i2c-dev` interface and an in-memory mock.
//!
//! All drivers in this crate talk to an [`I2cBus`], so every one of them runs
//! unchanged against [`MockI2c`] on a development machine and in tests.

use crate::error::{HwError, Result};
use std::any::Any;
use std::collections::BTreeMap;

/// The Pi's header I²C bus (GPIO2/GPIO3).
pub const DEFAULT_BUS: u8 = 1;

/// Minimal I²C master operations used by PixelPlus drivers.
pub trait I2cBus: Send {
    /// Write `data` to device `addr` in one transaction.
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<()>;

    /// Read `buf.len()` bytes from device `addr`.
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<()>;

    /// Write `data` then read into `buf` with a repeated start (register read).
    fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> Result<()>;

    /// `true` if a device acknowledges at `addr` (a one-byte read, as
    /// `i2cdetect -r` does; safe for EEPROMs, sensors, RTCs and SSD1306).
    fn probe(&mut self, addr: u8) -> bool {
        let mut b = [0u8; 1];
        self.read(addr, &mut b).is_ok()
    }
}

impl<T: I2cBus + ?Sized> I2cBus for Box<T> {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<()> {
        (**self).write(addr, data)
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<()> {
        (**self).read(addr, buf)
    }

    fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> Result<()> {
        (**self).write_read(addr, data, buf)
    }

    fn probe(&mut self, addr: u8) -> bool {
        (**self).probe(addr)
    }
}

fn check_addr(addr: u8) -> Result<()> {
    if !(0x03..=0x77).contains(&addr) {
        return Err(HwError::InvalidArgument(format!(
            "0x{addr:02x} is not a valid 7-bit I2C device address"
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub use linux::LinuxI2c;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};

    const I2C_RDWR: u64 = 0x0707;
    const I2C_M_RD: u16 = 0x0001;
    /// Kernel limit for one i2c-dev message.
    const MAX_MSG: usize = 8192;

    #[repr(C)]
    struct I2cMsg {
        addr: u16,
        flags: u16,
        len: u16,
        buf: *mut u8,
    }

    #[repr(C)]
    struct I2cRdwrData {
        msgs: *mut I2cMsg,
        nmsgs: u32,
    }

    /// An `/dev/i2c-N` bus using combined `I2C_RDWR` transfers.
    ///
    /// `I2C_RDWR` addresses each message explicitly, so it also works for
    /// devices a kernel driver has bound (shown as `UU` by `i2cdetect`).
    #[derive(Debug)]
    pub struct LinuxI2c {
        file: File,
        path: PathBuf,
    }

    impl LinuxI2c {
        /// Open bus `n` (`/dev/i2c-n`). Needs the `i2c-dev` module and
        /// `dtparam=i2c_arm=on` for bus 1.
        pub fn open(n: u8) -> Result<LinuxI2c> {
            Self::open_path(format!("/dev/i2c-{n}"))
        }

        /// Open a specific device node.
        pub fn open_path(path: impl AsRef<Path>) -> Result<LinuxI2c> {
            let path = path.as_ref().to_path_buf();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| {
                    let hint = if e.kind() == std::io::ErrorKind::NotFound {
                        " (enable I2C with dtparam=i2c_arm=on and load i2c-dev)"
                    } else {
                        ""
                    };
                    HwError::io(format!("opening {}{hint}", path.display()), e)
                })?;
            Ok(LinuxI2c { file, path })
        }

        /// The device node in use.
        pub fn path(&self) -> &Path {
            &self.path
        }

        fn transfer(&mut self, addr: u8, msgs: &mut [I2cMsg]) -> Result<()> {
            check_addr(addr)?;
            let mut data = I2cRdwrData {
                msgs: msgs.as_mut_ptr(),
                nmsgs: msgs.len() as u32,
            };
            // SAFETY: `data` points at `msgs`, whose buffers are live slices of
            // the stated lengths for the duration of the call.
            let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_RDWR as _, &mut data) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                return Err(HwError::i2c(addr, err.to_string()));
            }
            Ok(())
        }
    }

    fn msg(addr: u8, flags: u16, buf: *mut u8, len: usize) -> Result<I2cMsg> {
        if len > MAX_MSG {
            return Err(HwError::InvalidArgument(format!(
                "I2C message of {len} bytes exceeds {MAX_MSG}"
            )));
        }
        Ok(I2cMsg {
            addr: u16::from(addr),
            flags,
            len: len as u16,
            buf,
        })
    }

    impl I2cBus for LinuxI2c {
        fn write(&mut self, addr: u8, data: &[u8]) -> Result<()> {
            // The kernel only reads from a write message's buffer.
            let mut msgs = [msg(addr, 0, data.as_ptr() as *mut u8, data.len())?];
            self.transfer(addr, &mut msgs)
        }

        fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<()> {
            let mut msgs = [msg(addr, I2C_M_RD, buf.as_mut_ptr(), buf.len())?];
            self.transfer(addr, &mut msgs)
        }

        fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> Result<()> {
            let mut msgs = [
                msg(addr, 0, data.as_ptr() as *mut u8, data.len())?,
                msg(addr, I2C_M_RD, buf.as_mut_ptr(), buf.len())?,
            ];
            self.transfer(addr, &mut msgs)
        }
    }
}

/// A simulated I²C device for [`MockI2c`].
pub trait MockDevice: Send + Any {
    /// Handle a write transaction.
    fn write(&mut self, data: &[u8]) -> Result<()>;
    /// Handle a read transaction.
    fn read(&mut self, buf: &mut [u8]) -> Result<()>;
    /// Downcast support for tests.
    fn as_any(&self) -> &dyn Any;
    /// Downcast support for tests.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// A bus of simulated devices keyed by address; absent addresses NACK.
#[derive(Default)]
pub struct MockI2c {
    devices: BTreeMap<u8, Box<dyn MockDevice>>,
}

impl std::fmt::Debug for MockI2c {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockI2c")
            .field("addresses", &self.devices.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MockI2c {
    /// An empty bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a device.
    pub fn add(&mut self, addr: u8, device: impl MockDevice) -> &mut Self {
        self.devices.insert(addr, Box::new(device));
        self
    }

    /// Builder form of [`MockI2c::add`].
    pub fn with(mut self, addr: u8, device: impl MockDevice) -> Self {
        self.add(addr, device);
        self
    }

    /// Remove the device at `addr`.
    pub fn remove(&mut self, addr: u8) {
        self.devices.remove(&addr);
    }

    /// Addresses with a device.
    pub fn addresses(&self) -> Vec<u8> {
        self.devices.keys().copied().collect()
    }

    /// Borrow the device at `addr` as its concrete type.
    pub fn device<T: MockDevice>(&self, addr: u8) -> Option<&T> {
        self.devices.get(&addr)?.as_any().downcast_ref()
    }

    /// Mutably borrow the device at `addr` as its concrete type.
    pub fn device_mut<T: MockDevice>(&mut self, addr: u8) -> Option<&mut T> {
        self.devices.get_mut(&addr)?.as_any_mut().downcast_mut()
    }

    fn dev(&mut self, addr: u8) -> Result<&mut Box<dyn MockDevice>> {
        check_addr(addr)?;
        self.devices
            .get_mut(&addr)
            .ok_or_else(|| HwError::i2c(addr, "no acknowledge (simulated bus)"))
    }
}

impl I2cBus for MockI2c {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<()> {
        self.dev(addr)?.write(data)
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<()> {
        self.dev(addr)?.read(buf)
    }

    fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> Result<()> {
        let dev = self.dev(addr)?;
        dev.write(data)?;
        dev.read(buf)
    }
}

/// Byte-addressed register file with an auto-incrementing pointer
/// (DS3231, AT24Cxx with a 2-byte pointer).
#[derive(Debug, Clone)]
pub struct MockByteRegisters {
    /// Register / memory contents.
    pub mem: Vec<u8>,
    pointer_bytes: usize,
    ptr: usize,
    /// Number of write transactions that carried data (not just a pointer).
    pub data_writes: usize,
}

impl MockByteRegisters {
    /// `size` bytes (filled with `fill`) addressed by a `pointer_bytes`-byte
    /// big-endian pointer.
    pub fn new(size: usize, pointer_bytes: usize, fill: u8) -> Self {
        MockByteRegisters {
            mem: vec![fill; size.max(1)],
            pointer_bytes: pointer_bytes.clamp(1, 2),
            ptr: 0,
            data_writes: 0,
        }
    }
}

impl MockDevice for MockByteRegisters {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        if data.len() < self.pointer_bytes {
            return Ok(()); // a quick write / probe
        }
        let (p, rest) = data.split_at(self.pointer_bytes);
        self.ptr = p.iter().fold(0usize, |acc, &b| (acc << 8) | usize::from(b)) % self.mem.len();
        if !rest.is_empty() {
            self.data_writes += 1;
        }
        for &b in rest {
            self.mem[self.ptr] = b;
            self.ptr = (self.ptr + 1) % self.mem.len();
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        for b in buf {
            *b = self.mem[self.ptr];
            self.ptr = (self.ptr + 1) % self.mem.len();
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 16-bit big-endian registers selected by a one-byte pointer (LM75, INA226).
#[derive(Debug, Clone)]
pub struct MockWordRegisters {
    /// Register values.
    pub regs: BTreeMap<u8, u16>,
    ptr: u8,
    /// Registers written, in order: `(register, value)`.
    pub written: Vec<(u8, u16)>,
}

impl MockWordRegisters {
    /// A device with the given initial register values.
    pub fn new(regs: &[(u8, u16)]) -> Self {
        MockWordRegisters {
            regs: regs.iter().copied().collect(),
            ptr: 0,
            written: Vec::new(),
        }
    }

    /// An LM75B reading `celsius`.
    pub fn lm75(celsius: f64) -> Self {
        let raw = ((celsius / 0.125).round() as i16) << 5;
        Self::new(&[
            (0x00, raw as u16),
            (0x01, 0x0000),
            (0x02, 0x4B00),
            (0x03, 0x5000),
        ])
    }

    /// An INA226 on a 10 mΩ shunt measuring `volts` and `amps` (power-on
    /// register values; the driver programs calibration itself).
    pub fn ina226(volts: f64, amps: f64) -> Self {
        let bus = (volts / 0.00125).round() as u16;
        let shunt = ((amps * 0.010) / 0.0000025).round() as i16 as u16;
        let mut dev = Self::new(&[
            (0x00, 0x4127),
            (0x01, shunt),
            (0x02, bus),
            (0x05, 0),
            (0xFE, 0x5449),
            (0xFF, 0x2260),
        ]);
        dev.update_ina226();
        dev
    }

    /// Recompute INA226 current/power registers from shunt, bus and calibration.
    fn update_ina226(&mut self) {
        if self.regs.get(&0xFF) != Some(&0x2260) {
            return;
        }
        let cal = i64::from(*self.regs.get(&0x05).unwrap_or(&0));
        let shunt = i64::from(*self.regs.get(&0x01).unwrap_or(&0) as i16);
        let bus = i64::from(*self.regs.get(&0x02).unwrap_or(&0));
        let current = (shunt * cal) / 2048;
        let power = (current.abs() * bus) / 20_000;
        self.regs
            .insert(0x04, current.clamp(-32768, 32767) as i16 as u16);
        self.regs.insert(0x03, power.clamp(0, 65535) as u16);
    }
}

impl MockDevice for MockWordRegisters {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        if let Some((&p, rest)) = data.split_first() {
            self.ptr = p;
            if rest.len() >= 2 {
                let v = u16::from_be_bytes([rest[0], rest[1]]);
                self.regs.insert(p, v);
                self.written.push((p, v));
                self.update_ina226();
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        let v = self.regs.get(&self.ptr).copied().unwrap_or(0).to_be_bytes();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = v.get(i).copied().unwrap_or(0);
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Records every transaction (e.g. an SSD1306, which is write-only).
#[derive(Debug, Clone, Default)]
pub struct MockRecorder {
    /// Write transactions in order.
    pub writes: Vec<Vec<u8>>,
}

impl MockDevice for MockRecorder {
    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writes.push(data.to_vec());
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<()> {
        buf.fill(0);
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_bus_routing() {
        let mut bus = MockI2c::new().with(0x50, MockByteRegisters::new(256, 2, 0xFF));
        assert!(bus.probe(0x50));
        assert!(!bus.probe(0x51));
        assert!(bus.write(0x78, &[0]).is_err());
        bus.write(0x50, &[0x00, 0x10, 1, 2, 3]).unwrap();
        let mut out = [0u8; 3];
        bus.write_read(0x50, &[0x00, 0x10], &mut out).unwrap();
        assert_eq!(out, [1, 2, 3]);
        assert_eq!(
            bus.device::<MockByteRegisters>(0x50).unwrap().data_writes,
            1
        );
        assert!(bus.device::<MockRecorder>(0x50).is_none());
        assert_eq!(bus.addresses(), vec![0x50]);
    }

    #[test]
    fn word_registers() {
        let mut dev = MockWordRegisters::lm75(25.5);
        let mut buf = [0u8; 2];
        dev.write(&[0]).unwrap();
        dev.read(&mut buf).unwrap();
        let raw = i16::from_be_bytes(buf) >> 5;
        assert_eq!(f64::from(raw) * 0.125, 25.5);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_linux_bus_is_an_error() {
        let err = LinuxI2c::open_path("/nonexistent/i2c-9").unwrap_err();
        assert!(err.to_string().contains("i2c"));
    }
}
