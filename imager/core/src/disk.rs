//! Partition table parsing, a byte-range view of a partition, a sector-aligned I/O
//! adapter for raw devices, and writing `pixelplus.txt` into the FAT boot partition.
//!
//! The boot partition is modified *in place* through the same handle that wrote the
//! image (no mounting, works identically on Linux, macOS and Windows).

use std::io::{self, Read, Seek, SeekFrom, Write};

use crate::settings::{render_into, ImagerSettings, TEMPLATE};

pub const SECTOR: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub index: usize,
    pub kind: u8,
    pub start: u64, // bytes
    pub len: u64,   // bytes
}

/// Read the MBR partition table (Raspberry Pi OS images use MBR). Returns primary
/// partitions with non-zero type.
pub fn read_mbr<D: Read + Seek>(dev: &mut D) -> io::Result<Vec<Partition>> {
    let mut mbr = [0u8; 512];
    dev.seek(SeekFrom::Start(0))?;
    dev.read_exact(&mut mbr)?;
    if mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "no MBR boot signature (not a Raspberry Pi image?)",
        ));
    }
    let mut parts = Vec::new();
    for i in 0..4 {
        let e = &mbr[446 + i * 16..446 + (i + 1) * 16];
        let kind = e[4];
        let lba = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
        let count = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
        if kind != 0 && count != 0 {
            parts.push(Partition {
                index: i + 1,
                kind,
                start: lba * SECTOR,
                len: count * SECTOR,
            });
        }
    }
    Ok(parts)
}

/// First FAT partition (types 0x0b/0x0c FAT32, 0x0e/0x06/0x04/0x01 FAT16/12).
pub fn boot_partition(parts: &[Partition]) -> Option<Partition> {
    parts
        .iter()
        .copied()
        .find(|p| matches!(p.kind, 0x0b | 0x0c | 0x0e | 0x06 | 0x04 | 0x01))
}

/// A window `[start, start+len)` of an underlying stream.
pub struct Slice<T> {
    inner: T,
    start: u64,
    len: u64,
    pos: u64,
}

impl<T: Seek> Slice<T> {
    pub fn new(mut inner: T, start: u64, len: u64) -> io::Result<Self> {
        inner.seek(SeekFrom::Start(start))?;
        Ok(Slice {
            inner,
            start,
            len,
            pos: 0,
        })
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Read + Seek> Read for Slice<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let r = self.inner.read(&mut buf[..n])?;
        self.pos += r as u64;
        Ok(r)
    }
}

impl<T: Write + Seek> Write for Slice<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 && !buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write past end of partition",
            ));
        }
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let w = self.inner.write(&buf[..n])?;
        self.pos += w as u64;
        Ok(w)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<T> Seek for Slice<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if new < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = new as u64;
        Ok(self.pos)
    }
}

/// Makes arbitrary reads/writes sector-aligned (raw devices on Windows and macOS
/// `/dev/rdiskN` reject unaligned I/O). Caches one block; dirty data is written back on
/// `flush`, on block change, and on drop.
pub struct Aligned<T: Read + Write + Seek> {
    inner: T,
    block: u64,
    buf: Vec<u8>,
    cached: Option<u64>, // block index held in buf
    dirty: bool,
    pos: u64,
}

impl<T: Read + Write + Seek> Aligned<T> {
    pub fn new(inner: T, block: u64) -> Self {
        assert!(block >= SECTOR && block.is_power_of_two());
        Aligned {
            inner,
            block,
            buf: vec![0; block as usize],
            cached: None,
            dirty: false,
            pos: 0,
        }
    }

    fn load(&mut self, idx: u64) -> io::Result<()> {
        if self.cached == Some(idx) {
            return Ok(());
        }
        self.writeback()?;
        self.inner.seek(SeekFrom::Start(idx * self.block))?;
        let mut filled = 0;
        while filled < self.buf.len() {
            match self.inner.read(&mut self.buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        self.buf[filled..].fill(0);
        self.cached = Some(idx);
        Ok(())
    }

    fn writeback(&mut self) -> io::Result<()> {
        if let (true, Some(idx)) = (self.dirty, self.cached) {
            self.inner.seek(SeekFrom::Start(idx * self.block))?;
            self.inner.write_all(&self.buf)?;
            self.dirty = false;
        }
        Ok(())
    }
}

impl<T: Read + Write + Seek> Drop for Aligned<T> {
    fn drop(&mut self) {
        let _ = self.writeback();
        let _ = self.inner.flush();
    }
}

impl<T: Read + Write + Seek> Read for Aligned<T> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let idx = self.pos / self.block;
        let off = (self.pos % self.block) as usize;
        self.load(idx)?;
        let n = out.len().min(self.buf.len() - off);
        out[..n].copy_from_slice(&self.buf[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<T: Read + Write + Seek> Write for Aligned<T> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let idx = self.pos / self.block;
        let off = (self.pos % self.block) as usize;
        self.load(idx)?;
        let n = data.len().min(self.buf.len() - off);
        self.buf[off..off + n].copy_from_slice(&data[..n]);
        self.dirty = true;
        self.pos += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writeback()?;
        self.inner.flush()
    }
}

impl<T: Read + Write + Seek> Seek for Aligned<T> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => (self.pos as i64 + d).max(0) as u64,
            SeekFrom::End(_) => {
                // Device size is known to the caller; not needed by fatfs through Slice.
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "SeekFrom::End on raw device",
                ));
            }
        };
        Ok(self.pos)
    }
}

pub const SETTINGS_FILE: &str = "pixelplus.txt";

/// Result of customising the boot partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Injected {
    pub partition: Partition,
    pub used_existing_template: bool,
    pub bytes: usize,
}

/// Write `pixelplus.txt` (rendered from `settings`) into the FAT boot partition of
/// `dev`, which is a whole disk or a whole `.img` file. The existing file on the
/// partition is used as the template so its comments match the image version.
pub fn inject_settings<D: Read + Write + Seek>(
    dev: D,
    settings: &ImagerSettings,
) -> io::Result<Injected> {
    let mut dev = dev;
    let parts = read_mbr(&mut dev)?;
    let part = boot_partition(&parts)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no FAT boot partition found"))?;
    let slice = Slice::new(dev, part.start, part.len)?;
    let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new())?;
    let root = fs.root_dir();

    let mut existing = String::new();
    let mut used_existing = false;
    if let Ok(mut f) = root.open_file(SETTINGS_FILE) {
        let mut raw = Vec::new();
        f.read_to_end(&mut raw)?;
        if let Ok(s) = String::from_utf8(raw) {
            if !s.trim().is_empty() {
                existing = s;
                used_existing = true;
            }
        }
    }
    let template = if used_existing {
        existing.as_str()
    } else {
        TEMPLATE
    };
    let text = render_into(template, settings);

    let mut f = root.create_file(SETTINGS_FILE)?;
    f.truncate()?;
    f.write_all(text.as_bytes())?;
    f.flush()?;
    drop(f);
    drop(root);
    fs.unmount()?;
    Ok(Injected {
        partition: part,
        used_existing_template: used_existing,
        bytes: text.len(),
    })
}

/// Read `pixelplus.txt` back from an image or disk (used by verification and tests).
pub fn read_settings_file<D: Read + Write + Seek>(dev: D) -> io::Result<Option<String>> {
    let mut dev = dev;
    let parts = read_mbr(&mut dev)?;
    let part = boot_partition(&parts)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no FAT boot partition found"))?;
    let slice = Slice::new(dev, part.start, part.len)?;
    let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new())?;
    let root = fs.root_dir();
    let r = match root.open_file(SETTINGS_FILE) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Some(s)
        }
        Err(_) => None,
    };
    drop(root);
    fs.unmount()?;
    Ok(r)
}

#[cfg(test)]
pub(crate) mod testimg {
    use super::*;
    use std::io::Cursor;

    /// Build a tiny disk image: MBR + FAT32 partition at 4 MiB (like Raspberry Pi OS)
    /// + a dummy "rootfs" partition.
    pub fn make_image(with_template: Option<&str>) -> Vec<u8> {
        let boot_start = 4 * 1024 * 1024u64;
        let boot_len = 40 * 1024 * 1024u64; // FAT32 needs >= ~33 MiB with 512 B clusters
        let root_len = 1024 * 1024u64;
        let mut img = vec![0u8; (boot_start + boot_len + root_len) as usize];
        // MBR
        let mut e = |i: usize, kind: u8, start: u64, len: u64| {
            let b = 446 + i * 16;
            img[b + 4] = kind;
            img[b + 8..b + 12].copy_from_slice(&((start / SECTOR) as u32).to_le_bytes());
            img[b + 12..b + 16].copy_from_slice(&((len / SECTOR) as u32).to_le_bytes());
        };
        e(0, 0x0c, boot_start, boot_len);
        e(1, 0x83, boot_start + boot_len, root_len);
        img[510] = 0x55;
        img[511] = 0xAA;
        {
            let mut cur = Cursor::new(&mut img);
            let mut part = Slice::new(&mut cur, boot_start, boot_len).unwrap();
            fatfs::format_volume(
                &mut part,
                fatfs::FormatVolumeOptions::new()
                    .fat_type(fatfs::FatType::Fat32)
                    .volume_label(*b"bootfs     "),
            )
            .unwrap();
            let fs = fatfs::FileSystem::new(part, fatfs::FsOptions::new()).unwrap();
            {
                let root = fs.root_dir();
                root.create_file("config.txt")
                    .unwrap()
                    .write_all(b"arm_64bit=1\n")
                    .unwrap();
                if let Some(t) = with_template {
                    root.create_file(SETTINGS_FILE)
                        .unwrap()
                        .write_all(t.as_bytes())
                        .unwrap();
                }
            }
            fs.unmount().unwrap();
        }
        img
    }
}

#[cfg(test)]
mod tests {
    use super::testimg::make_image;
    use super::*;
    use crate::settings::parse_values;
    use std::io::Cursor;

    fn settings() -> ImagerSettings {
        ImagerSettings {
            wifi_ssid: "Home".into(),
            wifi_password: "hunter22!".into(),
            wifi_country: "US".into(),
            hostname: "pixelplus-porch".into(),
            ..Default::default()
        }
    }

    #[test]
    fn mbr_parse() {
        let img = make_image(None);
        let parts = read_mbr(&mut Cursor::new(&img)).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(boot_partition(&parts).unwrap().start, 4 * 1024 * 1024);
        assert!(read_mbr(&mut Cursor::new(vec![0u8; 512])).is_err());
    }

    #[test]
    fn inject_uses_existing_template() {
        let mut img = make_image(Some("# custom\nwifi_ssid=\nhostname=old\nboard=auto\n"));
        let r = inject_settings(Cursor::new(&mut img), &settings()).unwrap();
        assert!(r.used_existing_template);
        let text = read_settings_file(Cursor::new(&mut img)).unwrap().unwrap();
        assert!(
            text.starts_with("# custom\nwifi_ssid=Home\nhostname=pixelplus-porch\nboard=auto\n")
        );
        assert!(text.contains("\nwifi_password=hunter22!\n"));
        // other files untouched, image size untouched
        assert_eq!(img.len(), (45 * 1024 * 1024) as usize);
    }

    #[test]
    fn inject_without_file_uses_builtin_template() {
        let mut img = make_image(None);
        let r = inject_settings(Cursor::new(&mut img), &settings()).unwrap();
        assert!(!r.used_existing_template);
        let text = read_settings_file(Cursor::new(&mut img)).unwrap().unwrap();
        let vals = parse_values(&text);
        assert!(vals.contains(&("wifi_ssid".into(), "Home".into())));
        assert!(text.contains("PixelPlus settings"));
    }

    #[test]
    fn inject_twice_replaces() {
        let mut img = make_image(None);
        inject_settings(Cursor::new(&mut img), &settings()).unwrap();
        let mut s2 = settings();
        s2.hostname = "second".into();
        inject_settings(Cursor::new(&mut img), &s2).unwrap();
        let text = read_settings_file(Cursor::new(&mut img)).unwrap().unwrap();
        assert!(text.contains("\nhostname=second\n"));
        assert_eq!(text.matches("hostname=").count(), 1);
    }

    /// A stream that fails on any unaligned access, like a Windows physical drive.
    struct Strict(Cursor<Vec<u8>>);
    impl Read for Strict {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            assert_eq!(self.0.position() % SECTOR, 0, "unaligned read offset");
            assert_eq!(b.len() as u64 % SECTOR, 0, "unaligned read length");
            self.0.read(b)
        }
    }
    impl Write for Strict {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            assert_eq!(self.0.position() % SECTOR, 0, "unaligned write offset");
            assert_eq!(b.len() as u64 % SECTOR, 0, "unaligned write length");
            self.0.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for Strict {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.0.seek(p)
        }
    }

    #[test]
    fn inject_through_aligned_adapter() {
        let img = make_image(None);
        let mut strict = Strict(Cursor::new(img));
        {
            let dev = Aligned::new(&mut strict, 4096);
            inject_settings(dev, &settings()).unwrap();
        }
        let mut img = strict.0.into_inner();
        let text = read_settings_file(Cursor::new(&mut img)).unwrap().unwrap();
        assert!(text.contains("wifi_ssid=Home"));
    }

    #[test]
    fn slice_bounds() {
        let mut data = vec![0u8; 100];
        let mut s = Slice::new(Cursor::new(&mut data), 10, 20).unwrap();
        s.write_all(&[1u8; 20]).unwrap();
        assert!(s.write(&[1]).is_err());
        s.seek(SeekFrom::Start(0)).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 20);
        assert_eq!(data[9], 0);
        assert_eq!(data[10], 1);
        assert_eq!(data[30], 0);
    }
}
