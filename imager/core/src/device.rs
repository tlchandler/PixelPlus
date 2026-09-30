//! Opening a removable drive for raw writing, per operating system. Needs
//! administrator/root rights (the GUI runs this through the elevated helper):
//!
//! * Linux: unmount the card's partitions, open `/dev/sdX` with `O_EXCL` (fails if still
//!   mounted), flush the kernel buffer cache (`BLKFLSBUF`) before verifying, re-read the
//!   partition table (`BLKRRPART`) at the end.
//! * macOS: `diskutil unmountDisk`, write the *raw* node `/dev/rdiskN` (sector-aligned
//!   I/O only - see [`crate::disk::Aligned`]), `diskutil eject` at the end.
//! * Windows: lock + dismount every volume on the disk (`FSCTL_LOCK_VOLUME`,
//!   `FSCTL_DISMOUNT_VOLUME`, handles kept open while writing), then write
//!   `\\.\PhysicalDriveN` with write-through, sector-aligned I/O.

use std::fs::File;
use std::io;

use crate::drives::Drive;

pub struct OpenDevice {
    pub file: File,
    pub size: u64,
    /// Preferred I/O block (bytes, power of two >= 512).
    pub block: u64,
    #[cfg(windows)]
    _locks: Vec<win::VolumeLock>,
}

fn run(cmd: &str, args: &[&str]) -> io::Result<()> {
    let st = std::process::Command::new(cmd).args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{cmd} {} failed ({st})", args.join(" "))))
    }
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
pub fn open(drive: &Drive) -> io::Result<OpenDevice> {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::OpenOptionsExt;
    // Unmount everything on the card (desktop auto-mounters love to mount "bootfs").
    for mp in &drive.mountpoints {
        if run("umount", &[mp]).is_err() {
            let _ = run("umount", &["-l", mp]);
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_EXCL | libc::O_CLOEXEC)
        .open(&drive.device)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot open {}: {e} (is it still mounted?)", drive.device)))?;
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    Ok(OpenDevice { file, size, block: 4096 })
}

#[cfg(target_os = "linux")]
pub fn drop_caches(dev: &OpenDevice) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    dev.file.sync_all()?;
    const BLKFLSBUF: libc::c_ulong = 0x1261;
    // SAFETY: ioctl on a valid, open block-device descriptor with no argument pointer.
    let r = unsafe { libc::ioctl(dev.file.as_raw_fd(), BLKFLSBUF as _, 0) };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: plain advisory call on a valid descriptor.
    unsafe { libc::posix_fadvise(dev.file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn finish(dev: OpenDevice, _drive: &Drive) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    dev.file.sync_all()?;
    const BLKRRPART: libc::c_ulong = 0x125f;
    // SAFETY: as above; failure (e.g. busy) is harmless.
    unsafe { libc::ioctl(dev.file.as_raw_fd(), BLKRRPART as _, 0) };
    drop(dev);
    // SAFETY: global sync, no arguments.
    unsafe { libc::sync() };
    Ok(())
}

// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
fn whole_disk(dev: &str) -> String {
    // /dev/rdisk4 -> /dev/disk4
    dev.replacen("/dev/rdisk", "/dev/disk", 1)
}

#[cfg(target_os = "macos")]
pub fn open(drive: &Drive) -> io::Result<OpenDevice> {
    run("/usr/sbin/diskutil", &["unmountDisk", &whole_disk(&drive.device)])?;
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&drive.device)?;
    Ok(OpenDevice { file, size: drive.size, block: 4096 })
}

#[cfg(target_os = "macos")]
pub fn drop_caches(dev: &OpenDevice) -> io::Result<()> {
    // raw (/dev/rdisk) access is uncached
    dev.file.sync_all()
}

#[cfg(target_os = "macos")]
pub fn finish(dev: OpenDevice, drive: &Drive) -> io::Result<()> {
    dev.file.sync_all()?;
    drop(dev);
    let _ = run("/usr/sbin/diskutil", &["eject", &whole_disk(&drive.device)]);
    Ok(())
}

// ---------------------------------------------------------------------------
#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Ioctl::{FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    pub struct VolumeLock(#[allow(dead_code)] OwnedHandle);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn ioctl(h: &OwnedHandle, code: u32) -> io::Result<()> {
        let mut ret = 0u32;
        // SAFETY: valid handle; no in/out buffers for these FSCTLs.
        let ok = unsafe {
            DeviceIoControl(
                h.as_raw_handle() as _,
                code,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut ret,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Lock and dismount volume `E:` so the raw disk can be overwritten.
    pub fn lock_volume(letter: &str) -> io::Result<VolumeLock> {
        let path = wide(&format!(r"\\.\{}", letter.trim_end_matches('\\')));
        // SAFETY: path is NUL-terminated; other pointers null as documented.
        let h = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: h is a fresh valid handle we own.
        let h = unsafe { OwnedHandle::from_raw_handle(h as _) };
        let mut tries = 0;
        while let Err(e) = ioctl(&h, FSCTL_LOCK_VOLUME) {
            tries += 1;
            if tries > 10 {
                return Err(io::Error::new(e.kind(), format!("cannot lock {letter}: close any window showing the card ({e})")));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        ioctl(&h, FSCTL_DISMOUNT_VOLUME)?;
        Ok(VolumeLock(h))
    }
}

#[cfg(windows)]
pub fn open(drive: &Drive) -> io::Result<OpenDevice> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE};
    let mut locks = Vec::new();
    for l in &drive.mountpoints {
        locks.push(win::lock_volume(l)?);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_WRITE_THROUGH)
        .open(&drive.device)?;
    Ok(OpenDevice { file, size: drive.size, block: 4096, _locks: locks })
}

#[cfg(windows)]
pub fn drop_caches(dev: &OpenDevice) -> io::Result<()> {
    dev.file.sync_all()
}

#[cfg(windows)]
pub fn finish(dev: OpenDevice, _drive: &Drive) -> io::Result<()> {
    dev.file.sync_all()?;
    drop(dev); // releases the volume locks; Windows re-mounts the new partitions
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn open(_drive: &Drive) -> io::Result<OpenDevice> {
    Err(io::Error::other("raw disk writing is not supported on this OS"))
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn drop_caches(_dev: &OpenDevice) -> io::Result<()> {
    Ok(())
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn finish(_dev: OpenDevice, _drive: &Drive) -> io::Result<()> {
    Ok(())
}
