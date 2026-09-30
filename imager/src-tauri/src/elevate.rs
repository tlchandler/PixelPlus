//! Start the write helper with administrator rights.
//!
//! * Linux: `pkexec <app> --helper write JOB --progress FILE`. For the AppImage the helper
//!   is the `.AppImage` file itself (`$APPIMAGE`) because root cannot read the user's FUSE
//!   mount of it. Already root (e.g. `sudo`)? Run directly.
//! * macOS: `osascript -e 'do shell script "..." with administrator privileges'` - the
//!   standard system password dialog; the helper writes `/dev/rdiskN`.
//! * Windows: the app itself runs elevated (`requireAdministrator` manifest), so the helper
//!   is a plain child process.

use std::path::{Path, PathBuf};

use tokio::process::{Child, Command};

pub fn helper_exe() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe()
}

fn helper_args(job: &Path, progress: &Path) -> Vec<String> {
    vec![
        "--helper".into(),
        "write".into(),
        job.display().to_string(),
        "--progress".into(),
        progress.display().to_string(),
    ]
}

/// POSIX shell single-quote.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Escape for an AppleScript string literal.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn spawn(job: &Path, progress: &Path) -> std::io::Result<Child> {
    let exe = helper_exe()?;
    let args = helper_args(job, progress);

    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        let is_root = unsafe { libc::geteuid() } == 0;
        let mut cmd = if is_root {
            let mut c = Command::new(&exe);
            c.args(&args);
            c
        } else {
            let mut c = Command::new("pkexec");
            c.arg(&exe).args(&args);
            c
        };
        cmd.kill_on_drop(false).spawn()
    }

    #[cfg(target_os = "macos")]
    {
        let line = std::iter::once(exe.display().to_string())
            .chain(args)
            .map(|a| sh_quote(&a))
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            "do shell script {} with prompt {} with administrator privileges",
            applescript_quote(&format!("{line} >/dev/null 2>&1")),
            applescript_quote(
                "PixelPlus Imager needs your permission to erase and write the SD card."
            )
        );
        Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .kill_on_drop(false)
            .spawn()
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = Command::new(&exe);
        c.args(&args).creation_flags(CREATE_NO_WINDOW);
        c.kill_on_drop(false).spawn()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (exe, args);
        Err(std::io::Error::other("unsupported operating system"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("a b'c"), r"'a b'\''c'");
        assert_eq!(applescript_quote(r#"x "y" \z"#), r#""x \"y\" \\z""#);
    }
}
