//! `pixelplus doctor`: check the system for the problems that stop a show.

use crate::cmd::detect::{level as sensor_level, read_sensors};
use crate::cmd::status::daemon_url;
use crate::http::{self, BaseUrl};
use crate::hwctx::HwContext;
use crate::style::{self, paint, Level};
use anyhow::{bail, Result};
use clap::Args;
use pixelplus_core::model::BoardKind;
use pixelplus_hw::board::DetectionSource;
use pixelplus_hw::eeprom::board_id;
use pixelplus_hw::rtc::Ds3231;
use pixelplus_output::DpiSoc;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Arguments of `doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Daemon URL to probe [default: http://127.0.0.1:$PIXELPLUS_HTTP_PORT].
    #[arg(long)]
    pub url: Option<String>,
}

/// One check result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Stable id.
    pub id: &'static str,
    /// Short label.
    pub label: &'static str,
    /// Outcome.
    pub status: Level,
    /// Explanation / remedy.
    pub detail: String,
}

fn check(id: &'static str, label: &'static str, status: Level, detail: impl Into<String>) -> Check {
    Check {
        id,
        label,
        status,
        detail: detail.into(),
    }
}

/// Active (uncommented) lines of config.txt.
pub fn config_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn find_config(root: &Path) -> Option<PathBuf> {
    ["boot/firmware/config.txt", "boot/config.txt"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file())
}

/// Checks on `config.txt` and the overlay files.
pub fn boot_checks(root: &Path, board: Option<BoardKind>, soc: Option<DpiSoc>) -> Vec<Check> {
    let mut out = Vec::new();
    let Some(path) = find_config(root) else {
        out.push(check(
            "configTxt",
            "Boot config",
            Level::Warn,
            "no /boot/firmware/config.txt (not a Raspberry Pi OS system?)",
        ));
        return out;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let lines = config_lines(&text);
    let has = |prefix: &str| lines.iter().any(|l| l.starts_with(prefix));
    out.push(if has("dtparam=i2c_arm=on") {
        check("i2cEnabled", "I2C enabled", Level::Ok, "dtparam=i2c_arm=on")
    } else {
        check(
            "i2cEnabled",
            "I2C enabled",
            Level::Fail,
            format!("add dtparam=i2c_arm=on to {}", path.display()),
        )
    });
    let needs_dpi = board.is_some_and(|b| b.output_count() > 0);
    if needs_dpi {
        let board_name = board.map(board_id).unwrap_or("difftx");
        out.push(if has("dtoverlay=pixelplus-dpi") {
            check(
                "dpiOverlay",
                "DPI overlay",
                Level::Ok,
                lines
                    .iter()
                    .find(|l| l.starts_with("dtoverlay=pixelplus-dpi"))
                    .cloned()
                    .unwrap_or_default(),
            )
        } else {
            check(
                "dpiOverlay",
                "DPI overlay",
                Level::Fail,
                format!(
                    "not in {}; run `pixelplus config-txt --board {board_name}` and add its output",
                    path.display()
                ),
            )
        });
        if let Some(soc) = soc {
            let dtbo = path
                .parent()
                .unwrap_or(root)
                .join("overlays")
                .join(format!("{}.dtbo", soc.overlay_name()));
            out.push(if dtbo.is_file() {
                check(
                    "overlayInstalled",
                    "Overlay file",
                    Level::Ok,
                    dtbo.display().to_string(),
                )
            } else {
                check(
                    "overlayInstalled",
                    "Overlay file",
                    Level::Fail,
                    format!(
                        "{} is missing (compile it from `pixelplus config-txt --overlay`)",
                        dtbo.display()
                    ),
                )
            });
        }
        out.push(if has("dtoverlay=vc4-kms-v3d") {
            check(
                "kms",
                "KMS display driver",
                Level::Ok,
                "dtoverlay=vc4-kms-v3d",
            )
        } else {
            check(
                "kms",
                "KMS display driver",
                Level::Warn,
                "dtoverlay=vc4-kms-v3d not found; the DPI overlay needs the KMS stack",
            )
        });
    }
    out
}

/// Is a DPI connector registered with DRM (i.e. the overlay is loaded)?
pub fn dpi_connector(root: &Path) -> Option<String> {
    let dir = std::fs::read_dir(root.join("sys/class/drm")).ok()?;
    dir.filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|n| n.contains("-DPI-"))
}

fn audio_cards(root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(root.join("proc/asound/cards")).unwrap_or_default();
    text.lines()
        .filter(|l| {
            l.trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
        .filter_map(|l| l.split(" - ").nth(1).or_else(|| l.split(':').nth(1)))
        .map(|s| s.trim().to_string())
        .collect()
}

#[cfg(target_os = "linux")]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and a properly sized out-struct.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    // The field types are 32-bit on 32-bit ARM userlands; widen explicitly.
    #[allow(clippy::unnecessary_cast)]
    let free = st.f_bavail as u64 * st.f_frsize as u64;
    (rc == 0).then_some(free)
}

#[cfg(not(target_os = "linux"))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
}

fn time_sync(root: &Path) -> Check {
    let out = std::process::Command::new("timedatectl")
        .args(["show", "-p", "NTPSynchronized", "--value"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            if String::from_utf8_lossy(&o.stdout).trim() == "yes" {
                check(
                    "timeSync",
                    "Time sync",
                    Level::Ok,
                    "clock synchronised (NTP)",
                )
            } else {
                check(
                    "timeSync",
                    "Time sync",
                    Level::Warn,
                    "clock not synchronised: schedules and sunset times may be wrong",
                )
            }
        }
        _ if root.join("run/systemd/timesync/synchronized").exists() => check(
            "timeSync",
            "Time sync",
            Level::Ok,
            "clock synchronised (systemd-timesyncd)",
        ),
        _ => check(
            "timeSync",
            "Time sync",
            Level::Warn,
            "could not determine whether the clock is synchronised",
        ),
    }
}

fn throttling(root: &Path) -> Option<Check> {
    let raw =
        std::fs::read_to_string(root.join("sys/devices/platform/soc/soc:firmware/get_throttled"))
            .ok()?;
    let v = u32::from_str_radix(raw.trim().trim_start_matches("0x"), 16).ok()?;
    Some(if v & 0x1 != 0 {
        check(
            "power",
            "Pi power supply",
            Level::Fail,
            "under-voltage right now: use a better 5 V supply or cable",
        )
    } else if v & 0x10000 != 0 {
        check(
            "power",
            "Pi power supply",
            Level::Warn,
            "under-voltage has occurred since boot",
        )
    } else if v & 0x4 != 0 || v & 0x40000 != 0 {
        check(
            "power",
            "Pi power supply",
            Level::Warn,
            "the CPU has been throttled (temperature or supply)",
        )
    } else {
        check(
            "power",
            "Pi power supply",
            Level::Ok,
            "no under-voltage or throttling",
        )
    })
}

#[cfg(target_os = "linux")]
fn pinmux_check(soc: Option<DpiSoc>) -> Check {
    use pixelplus_output::pinmux::{PinMux, PinMuxMethod};
    let Some(soc) = soc else {
        return check("pinmux", "Pin control", Level::Warn, "not a Raspberry Pi");
    };
    match PinMux::detect(soc) {
        Ok(m) => match m.method() {
            PinMuxMethod::Pinctrl(p) => check(
                "pinmux",
                "Pin control",
                Level::Ok,
                format!("pinctrl ({})", p.display()),
            ),
            PinMuxMethod::GpioMem => check(
                "pinmux",
                "Pin control",
                Level::Warn,
                "using /dev/gpiomem; install raspi-utils for pinctrl",
            ),
        },
        Err(e) => check("pinmux", "Pin control", Level::Fail, e.to_string()),
    }
}

/// Run every check.
pub fn run_checks(ctx: &HwContext, root: &Path, url: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let pi = ctx.pi_info();
    let soc = pi.as_ref().and_then(|p| DpiSoc::from_model(&p.model));
    out.push(match &pi {
        Some(p) if p.family.is_supported() => check("pi", "Raspberry Pi", Level::Ok, &p.model),
        Some(p) => check(
            "pi",
            "Raspberry Pi",
            Level::Warn,
            format!("{} is not a supported model (Zero 2 W, 3, 4, 5)", p.model),
        ),
        None => check(
            "pi",
            "Raspberry Pi",
            Level::Warn,
            "not a Raspberry Pi: no pixel outputs or board sensors",
        ),
    });

    let detection = ctx.detect().ok();
    let board = detection.as_ref().and_then(|d| d.board.or(d.suggested));
    match &detection {
        Some(d) => {
            out.push(match (d.board, d.suggested) {
                (Some(b), _) => {
                    let src = if d.source == DetectionSource::FppEeprom { " (FPP cape EEPROM)" } else { "" };
                    let rev = d.rev.as_deref().map(|r| format!(" rev {r}")).unwrap_or_default();
                    check("board", "Board", Level::Ok, format!("{}{rev}{src}", b.display_name()))
                }
                (None, Some(s)) => check("board", "Board", Level::Warn, format!(
                    "EEPROM not programmed; looks like {}: `pixelplus eeprom write --board {} --rev <rev>`",
                    s.display_name(), board_id(s)
                )),
                (None, None) => check("board", "Board", Level::Warn, "no PixelPlus board detected (bare Pi or leader-only node)"),
            });
            for w in &d.warnings {
                out.push(check("boardWarning", "Board note", Level::Warn, w));
            }
        }
        None => out.push(check(
            "board",
            "Board",
            Level::Warn,
            "could not probe the board",
        )),
    }

    if !ctx.is_simulated() {
        out.extend(boot_checks(root, board, soc));
        if board.is_some_and(|b| b.output_count() > 0) {
            out.push(match dpi_connector(root) {
                Some(name) => check(
                    "dpiLoaded",
                    "DPI device",
                    Level::Ok,
                    format!("{name} registered"),
                ),
                None => check(
                    "dpiLoaded",
                    "DPI device",
                    Level::Fail,
                    "no DPI display connector; is the overlay in config.txt and did you reboot?",
                ),
            });
            #[cfg(target_os = "linux")]
            out.push(pinmux_check(soc));
        }
        out.push(if root.join("dev/i2c-1").exists() {
            check("i2cDev", "I2C bus", Level::Ok, "/dev/i2c-1")
        } else {
            check(
                "i2cDev",
                "I2C bus",
                Level::Fail,
                "/dev/i2c-1 missing: dtparam=i2c_arm=on and the i2c-dev module",
            )
        });
    }

    if let Some(b) = board {
        let sensors = read_sensors(ctx, b);
        let worst = sensors
            .iter()
            .map(|s| sensor_level(s.status()))
            .max()
            .unwrap_or(Level::Ok);
        let detail = if sensors.is_empty() {
            "no sensors readable".to_string()
        } else {
            sensors
                .iter()
                .map(|s| format!("{} {:.1} {}", s.label, s.value, s.unit))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push(check("sensors", "Sensors", worst, detail));
        if b == BoardKind::Difftxlarge {
            let rtc = ctx
                .bus()
                .ok()
                .map(|mut bus| Ds3231::new(bus.as_mut()).read_time());
            out.push(match rtc {
                Some(Ok(t)) if t.oscillator_stopped => check(
                    "rtc",
                    "RTC",
                    Level::Warn,
                    "DS3231 lost power (check the CR2032); it is re-set once time syncs",
                ),
                Some(Ok(t)) => check(
                    "rtc",
                    "RTC",
                    Level::Ok,
                    format!("DS3231 {} UTC", t.utc.format("%Y-%m-%d %H:%M:%S")),
                ),
                Some(Err(e)) => check("rtc", "RTC", Level::Warn, format!("DS3231 unreadable: {e}")),
                None => check("rtc", "RTC", Level::Warn, "no I2C bus"),
            });
        }
    }

    let cards = audio_cards(root);
    out.push(if cards.is_empty() {
        check(
            "audio",
            "Audio",
            Level::Warn,
            "no sound devices (Pi 5: use HDMI or a USB sound card)",
        )
    } else {
        check("audio", "Audio", Level::Ok, cards.join(", "))
    });

    let data_dir = ["var/lib/pixelplus", ""]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.exists())
        .unwrap_or_else(|| root.to_path_buf());
    if let Some(free) = free_bytes(&data_dir) {
        let gb = free as f64 / 1e9;
        out.push(match gb {
            g if g < 0.5 => check(
                "disk",
                "Disk space",
                Level::Fail,
                format!("{g:.2} GB free on {}", data_dir.display()),
            ),
            g if g < 2.0 => check(
                "disk",
                "Disk space",
                Level::Warn,
                format!("{g:.1} GB free on {}", data_dir.display()),
            ),
            g => check("disk", "Disk space", Level::Ok, format!("{g:.1} GB free")),
        });
    }
    out.push(time_sync(root));
    if let Some(c) = throttling(root) {
        out.push(c);
    }
    out.push(
        match BaseUrl::parse(url)
            .and_then(|b| http::get(&b, "/api/v1/system", Duration::from_secs(1)))
        {
            Ok(r) if r.status == 200 || r.status == 401 => check(
                "daemon",
                "pixelplusd",
                Level::Ok,
                format!("answering at {url}"),
            ),
            Ok(r) => check(
                "daemon",
                "pixelplusd",
                Level::Warn,
                format!("{url} answered HTTP {}", r.status),
            ),
            Err(_) => check(
                "daemon",
                "pixelplusd",
                Level::Warn,
                format!("not reachable at {url} (systemctl status pixelplusd)"),
            ),
        },
    );
    out
}

/// Run `doctor`.
pub fn run(ctx: &HwContext, args: DoctorArgs, json: bool) -> Result<()> {
    let url = daemon_url(args.url.as_deref());
    let checks = run_checks(ctx, Path::new("/"), &url);
    let count = |l: Level| checks.iter().filter(|c| c.status == l).count();
    let (ok, warn, fail) = (count(Level::Ok), count(Level::Warn), count(Level::Fail));
    if json {
        let out = serde_json::json!({ "ok": fail == 0, "checks": checks });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        style::heading("PixelPlus doctor");
        if ctx.is_simulated() {
            anstream::println!(
                "{}",
                paint(style::DIM, "(simulated board; boot checks skipped)")
            );
        }
        for c in &checks {
            anstream::println!("  {} {:<18} {}", c.status.badge(), c.label, c.detail);
        }
        anstream::println!(
            "\n{} ok, {} warning(s), {} problem(s)",
            paint(style::OK, ok),
            paint(style::WARN, warn),
            paint(if fail > 0 { style::FAIL } else { style::DIM }, fail)
        );
    }
    if fail > 0 {
        bail!("{fail} problem(s) found");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parsing_and_boot_checks() {
        let root = std::env::temp_dir().join(format!("pixelplus-doctor-{}", std::process::id()));
        let fw = root.join("boot/firmware");
        std::fs::create_dir_all(fw.join("overlays")).unwrap();
        std::fs::write(
            fw.join("config.txt"),
            "# dtoverlay=pixelplus-dpi\ndtparam=i2c_arm=on  # yes\ndtoverlay=vc4-kms-v3d\n[all]\n",
        )
        .unwrap();
        let checks = boot_checks(&root, Some(BoardKind::Difftx), Some(DpiSoc::Bcm2711));
        let status = |id: &str| checks.iter().find(|c| c.id == id).map(|c| c.status);
        assert_eq!(status("i2cEnabled"), Some(Level::Ok));
        assert_eq!(
            status("dpiOverlay"),
            Some(Level::Fail),
            "commented line must not count"
        );
        assert_eq!(status("overlayInstalled"), Some(Level::Fail));
        assert_eq!(status("kms"), Some(Level::Ok));

        std::fs::write(
            fw.join("config.txt"),
            "dtparam=i2c_arm=on\ndtoverlay=pixelplus-dpi,vactive=807\n",
        )
        .unwrap();
        std::fs::write(fw.join("overlays/pixelplus-dpi.dtbo"), b"x").unwrap();
        let checks = boot_checks(&root, Some(BoardKind::Difftx), Some(DpiSoc::Bcm2711));
        assert!(
            checks
                .iter()
                .filter(|c| c.id != "kms")
                .all(|c| c.status == Level::Ok),
            "{checks:?}"
        );
        // Bare Pi needs no overlay.
        let checks = boot_checks(&root, Some(BoardKind::BarePi), None);
        assert!(checks.iter().all(|c| c.id != "dpiOverlay"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sysfs_probes() {
        let root =
            std::env::temp_dir().join(format!("pixelplus-doctor-sys-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sys/class/drm/card1-DPI-1")).unwrap();
        std::fs::create_dir_all(root.join("proc/asound")).unwrap();
        std::fs::write(
            root.join("proc/asound/cards"),
            " 0 [Headphones     ]: bcm2835_headpho - bcm2835 Headphones\n                      bcm2835 Headphones\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("sys/devices/platform/soc/soc:firmware")).unwrap();
        std::fs::write(
            root.join("sys/devices/platform/soc/soc:firmware/get_throttled"),
            "0x50000\n",
        )
        .unwrap();
        assert_eq!(dpi_connector(&root).as_deref(), Some("card1-DPI-1"));
        assert_eq!(audio_cards(&root), vec!["bcm2835 Headphones"]);
        assert_eq!(throttling(&root).unwrap().status, Level::Warn);
        let empty = root.join("nothing");
        assert!(dpi_connector(&empty).is_none());
        assert!(audio_cards(&empty).is_empty());
        assert!(throttling(&empty).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
