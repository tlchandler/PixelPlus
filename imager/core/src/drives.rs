//! Listing removable drives safely.
//!
//! Rules (all platforms): only whole disks that are removable/hot-pluggable or on a
//! USB/SD/MMC bus; never a disk holding the running system (`/`, `/boot`, `/usr`,
//! swap, macOS internal media, Windows boot/system disks); nothing read-only; nothing
//! larger than [`MAX_SIZE`] (an SD card for a Pi, not a backup drive) unless
//! `PIXELPLUS_IMAGER_ALLOW_LARGE=1`.
//!
//! Parsers are pure functions over the platform tools' output (`lsblk -J`,
//! `diskutil ... -plist`, PowerShell `Get-Disk | ConvertTo-Json`) so they are unit-tested
//! on every OS.

use serde::{Deserialize, Serialize};

pub const MAX_SIZE: u64 = 1024 * 1024 * 1024 * 1024; // 1 TiB
pub const MIN_SIZE: u64 = 3_500_000_000; // a PixelPlus image needs a >= 4 GB card

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// What to open for raw writing: `/dev/sdb`, `/dev/rdisk4`, `\\.\PhysicalDrive2`.
    pub device: String,
    /// Human name: "Generic SD/MMC (31.9 GB)".
    pub name: String,
    pub size: u64,
    pub bus: String,
    pub mountpoints: Vec<String>,
    /// Too small for PixelPlus (still listed, but the UI disables it).
    pub too_small: bool,
}

fn allow_large() -> bool {
    std::env::var("PIXELPLUS_IMAGER_ALLOW_LARGE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn size_ok(size: u64) -> bool {
    size > 0 && (size <= MAX_SIZE || allow_large())
}

pub fn human_size(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", b as f64 / 1e6)
    }
}

const SYSTEM_MOUNTS: &[&str] = &[
    "/",
    "/boot",
    "/boot/efi",
    "/boot/firmware",
    "/usr",
    "/var",
    "/home",
    "/efi",
    "[SWAP]",
];

// ---------------------------------------------------------------------------
// Linux: lsblk -J -b -o NAME,PATH,SIZE,RM,HOTPLUG,TRAN,MODEL,VENDOR,TYPE,RO,MOUNTPOINT
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDev>,
}

#[derive(Deserialize)]
struct LsblkDev {
    name: String,
    path: Option<String>,
    size: Option<serde_json::Value>,
    rm: Option<serde_json::Value>,
    hotplug: Option<serde_json::Value>,
    tran: Option<String>,
    model: Option<String>,
    vendor: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    ro: Option<serde_json::Value>,
    mountpoint: Option<String>,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
    #[serde(default)]
    children: Vec<LsblkDev>,
}

fn jbool(v: &Option<serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s == "1" || s == "true",
        Some(serde_json::Value::Number(n)) => n.as_u64() == Some(1),
        _ => false,
    }
}

fn jnum(v: &Option<serde_json::Value>) -> u64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn lsblk_mounts(d: &LsblkDev, out: &mut Vec<String>) {
    if let Some(m) = &d.mountpoint {
        out.push(m.clone());
    }
    for m in d.mountpoints.iter().flatten() {
        if !out.contains(m) {
            out.push(m.clone());
        }
    }
    for c in &d.children {
        lsblk_mounts(c, out);
    }
}

pub fn parse_lsblk(json: &str) -> Result<Vec<Drive>, serde_json::Error> {
    let root: LsblkRoot = serde_json::from_str(json)?;
    let mut drives = Vec::new();
    for d in root.blockdevices {
        if d.kind.as_deref() != Some("disk") || jbool(&d.ro) {
            continue;
        }
        let tran = d.tran.clone().unwrap_or_default().to_lowercase();
        let removable =
            jbool(&d.rm) || jbool(&d.hotplug) || matches!(tran.as_str(), "usb" | "mmc" | "sd");
        // mmcblk0 on a Pi / laptop SD slot: removable media but may be the boot disk -> mount check below
        if !removable || d.name.starts_with("loop") || d.name.starts_with("zram") {
            continue;
        }
        let mut mounts = Vec::new();
        lsblk_mounts(&d, &mut mounts);
        if mounts
            .iter()
            .any(|m| SYSTEM_MOUNTS.contains(&m.as_str()) || m.starts_with("/snap"))
        {
            continue;
        }
        let size = jnum(&d.size);
        if !size_ok(size) {
            continue;
        }
        let label = [d.vendor.as_deref(), d.model.as_deref()]
            .iter()
            .flatten()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let label = if label.is_empty() {
            format!("{} drive", tran.to_uppercase())
        } else {
            label
        };
        drives.push(Drive {
            device: d.path.clone().unwrap_or_else(|| format!("/dev/{}", d.name)),
            name: format!("{label} ({})", human_size(size)),
            size,
            bus: if tran.is_empty() {
                "removable".into()
            } else {
                tran
            },
            mountpoints: mounts,
            too_small: size < MIN_SIZE,
        });
    }
    Ok(drives)
}

// ---------------------------------------------------------------------------
// macOS: `diskutil list -plist external physical` + `diskutil info -plist diskN`
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DuList {
    #[serde(default)]
    all_disks_and_partitions: Vec<DuDisk>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DuDisk {
    device_identifier: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    partitions: Vec<DuPart>,
    #[serde(default)]
    mount_point: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DuPart {
    #[serde(default)]
    mount_point: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct DuInfo {
    pub internal: bool,
    pub removable_media: bool,
    pub ejectable: bool,
    pub writable_media: bool,
    pub media_name: String,
    pub bus_protocol: String,
    #[serde(rename = "OSInternalMedia")]
    pub os_internal_media: bool,
    pub virtual_or_physical: String,
}

pub fn parse_diskutil_list(
    plist_xml: &[u8],
) -> Result<Vec<(String, u64, Vec<String>)>, plist::Error> {
    let l: DuList = plist::from_bytes(plist_xml)?;
    Ok(l.all_disks_and_partitions
        .into_iter()
        .map(|d| {
            let mut m: Vec<String> = d
                .partitions
                .iter()
                .filter_map(|p| p.mount_point.clone())
                .collect();
            if let Some(mp) = d.mount_point {
                m.push(mp);
            }
            (d.device_identifier, d.size, m)
        })
        .collect())
}

pub fn parse_diskutil_info(plist_xml: &[u8]) -> Result<DuInfo, plist::Error> {
    plist::from_bytes(plist_xml)
}

pub fn mac_drive(id: &str, size: u64, mounts: Vec<String>, info: &DuInfo) -> Option<Drive> {
    let system_mount = mounts
        .iter()
        .any(|m| m == "/" || m.starts_with("/System/Volumes"));
    if info.os_internal_media || system_mount || id == "disk0" || !info.writable_media {
        return None;
    }
    if info.virtual_or_physical == "Virtual"
        || !(info.removable_media || info.ejectable || !info.internal)
    {
        return None;
    }
    if !size_ok(size) {
        return None;
    }
    let name = if info.media_name.trim().is_empty() {
        "External drive".to_string()
    } else {
        info.media_name.trim().to_string()
    };
    Some(Drive {
        device: format!("/dev/r{id}"),
        name: format!("{name} ({})", human_size(size)),
        size,
        bus: info.bus_protocol.to_lowercase(),
        mountpoints: mounts,
        too_small: size < MIN_SIZE,
    })
}

// ---------------------------------------------------------------------------
// Windows: PowerShell Get-Disk (+ drive letters) as JSON
// ---------------------------------------------------------------------------

/// PowerShell used to enumerate disks (one JSON array, one object per disk).
pub const WINDOWS_PS: &str = r#"$ErrorActionPreference='Stop';
@(Get-Disk | ForEach-Object {
  $n = $_.Number
  $letters = @(Get-Partition -DiskNumber $n -ErrorAction SilentlyContinue | Where-Object DriveLetter | ForEach-Object { "$($_.DriveLetter):" })
  [pscustomobject]@{ Number=$n; FriendlyName=$_.FriendlyName; Size=[uint64]$_.Size; BusType="$($_.BusType)";
    IsBoot=$_.IsBoot; IsSystem=$_.IsSystem; IsReadOnly=$_.IsReadOnly; IsOffline=$_.IsOffline; Letters=$letters }
}) | ConvertTo-Json -Depth 3"#;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WinDisk {
    number: u32,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    bus_type: String,
    #[serde(default)]
    is_boot: bool,
    #[serde(default)]
    is_system: bool,
    #[serde(default)]
    is_read_only: bool,
    #[serde(default)]
    letters: Option<serde_json::Value>,
}

pub fn parse_windows(json: &str) -> Result<Vec<Drive>, serde_json::Error> {
    let t = json.trim();
    if t.is_empty() {
        return Ok(vec![]);
    }
    // ConvertTo-Json emits a bare object for a single disk
    let disks: Vec<WinDisk> = if t.starts_with('[') {
        serde_json::from_str(t)?
    } else {
        vec![serde_json::from_str(t)?]
    };
    let mut out = Vec::new();
    for d in disks {
        let bus = d.bus_type.to_lowercase();
        if d.is_boot
            || d.is_system
            || d.is_read_only
            || !matches!(bus.as_str(), "usb" | "sd" | "mmc")
        {
            continue;
        }
        if !size_ok(d.size) {
            continue;
        }
        let letters = match d.letters {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            Some(serde_json::Value::String(s)) => vec![s],
            _ => vec![],
        };
        let name = d.friendly_name.unwrap_or_else(|| "Removable drive".into());
        out.push(Drive {
            device: format!(r"\\.\PhysicalDrive{}", d.number),
            name: format!("{} ({})", name.trim(), human_size(d.size)),
            size: d.size,
            bus,
            mountpoints: letters,
            too_small: d.size < MIN_SIZE,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// run the platform tool
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error("could not run {tool}: {err}")]
    Tool { tool: &'static str, err: String },
    #[error("unexpected output from {tool}: {err}")]
    Parse { tool: &'static str, err: String },
}

#[cfg(target_os = "linux")]
pub fn list() -> Result<Vec<Drive>, ListError> {
    let out = std::process::Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,PATH,SIZE,RM,HOTPLUG,TRAN,MODEL,VENDOR,TYPE,RO,MOUNTPOINT",
        ])
        .output()
        .map_err(|e| ListError::Tool {
            tool: "lsblk",
            err: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(ListError::Tool {
            tool: "lsblk",
            err: String::from_utf8_lossy(&out.stderr).into(),
        });
    }
    parse_lsblk(&String::from_utf8_lossy(&out.stdout)).map_err(|e| ListError::Parse {
        tool: "lsblk",
        err: e.to_string(),
    })
}

#[cfg(target_os = "macos")]
pub fn list() -> Result<Vec<Drive>, ListError> {
    let run = |args: &[&str]| -> Result<Vec<u8>, ListError> {
        let o = std::process::Command::new("/usr/sbin/diskutil")
            .args(args)
            .output()
            .map_err(|e| ListError::Tool {
                tool: "diskutil",
                err: e.to_string(),
            })?;
        if !o.status.success() {
            return Err(ListError::Tool {
                tool: "diskutil",
                err: String::from_utf8_lossy(&o.stderr).into(),
            });
        }
        Ok(o.stdout)
    };
    let list =
        parse_diskutil_list(&run(&["list", "-plist", "external", "physical"])?).map_err(|e| {
            ListError::Parse {
                tool: "diskutil",
                err: e.to_string(),
            }
        })?;
    let mut drives = Vec::new();
    for (id, size, mounts) in list {
        let info =
            parse_diskutil_info(&run(&["info", "-plist", &id])?).map_err(|e| ListError::Parse {
                tool: "diskutil",
                err: e.to_string(),
            })?;
        if let Some(d) = mac_drive(&id, size, mounts, &info) {
            drives.push(d);
        }
    }
    Ok(drives)
}

#[cfg(windows)]
pub fn list() -> Result<Vec<Drive>, ListError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            WINDOWS_PS,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| ListError::Tool {
            tool: "powershell",
            err: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(ListError::Tool {
            tool: "powershell",
            err: String::from_utf8_lossy(&out.stderr).into(),
        });
    }
    parse_windows(&String::from_utf8_lossy(&out.stdout)).map_err(|e| ListError::Parse {
        tool: "powershell",
        err: e.to_string(),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn list() -> Result<Vec<Drive>, ListError> {
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices":[
      {"name":"nvme0n1","path":"/dev/nvme0n1","size":512110190592,"rm":false,"hotplug":false,"tran":"nvme","model":"Samsung SSD","vendor":null,"type":"disk","ro":false,"mountpoint":null,
        "children":[{"name":"nvme0n1p1","path":"/dev/nvme0n1p1","size":536870912,"rm":false,"hotplug":false,"tran":null,"model":null,"vendor":null,"type":"part","ro":false,"mountpoint":"/boot/efi"},
                    {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","size":511571000000,"rm":false,"hotplug":false,"tran":null,"model":null,"vendor":null,"type":"part","ro":false,"mountpoint":"/"}]},
      {"name":"sda","path":"/dev/sda","size":31914983424,"rm":true,"hotplug":true,"tran":"usb","model":"SD/MMC","vendor":"Generic ","type":"disk","ro":false,"mountpoint":null,
        "children":[{"name":"sda1","path":"/dev/sda1","size":536870912,"rm":true,"hotplug":true,"tran":null,"model":null,"vendor":null,"type":"part","ro":false,"mountpoint":"/media/me/bootfs"}]},
      {"name":"sdb","path":"/dev/sdb","size":4000787030016,"rm":false,"hotplug":true,"tran":"usb","model":"Backup Plus","vendor":"Seagate","type":"disk","ro":false,"mountpoint":null},
      {"name":"mmcblk0","path":"/dev/mmcblk0","size":2000000000,"rm":"1","hotplug":"0","tran":null,"model":null,"vendor":null,"type":"disk","ro":"0","mountpoint":null},
      {"name":"sr0","path":"/dev/sr0","size":1073741312,"rm":true,"hotplug":true,"tran":"sata","model":"DVD","vendor":null,"type":"rom","ro":false,"mountpoint":null},
      {"name":"sdc","path":"/dev/sdc","size":15931539456,"rm":true,"hotplug":true,"tran":"usb","model":"Locked","vendor":null,"type":"disk","ro":true,"mountpoint":null},
      {"name":"loop0","path":"/dev/loop0","size":100000000,"rm":false,"hotplug":false,"tran":null,"model":null,"vendor":null,"type":"loop","ro":true,"mountpoint":"/snap/core"}
    ]}"#;

    #[test]
    fn linux_filters_system_and_large_disks() {
        let d = parse_lsblk(LSBLK).unwrap();
        let devs: Vec<_> = d.iter().map(|x| x.device.as_str()).collect();
        assert_eq!(devs, vec!["/dev/sda", "/dev/mmcblk0"]);
        assert_eq!(d[0].name, "Generic SD/MMC (31.9 GB)");
        assert_eq!(d[0].mountpoints, vec!["/media/me/bootfs"]);
        assert!(!d[0].too_small);
        assert!(d[1].too_small);
    }

    #[test]
    fn linux_never_lists_a_booted_sd_card() {
        let j = r#"{"blockdevices":[{"name":"mmcblk0","path":"/dev/mmcblk0","size":31914983424,"rm":true,"hotplug":false,"tran":null,"type":"disk","ro":false,"mountpoint":null,
          "children":[{"name":"mmcblk0p1","type":"part","mountpoint":"/boot/firmware"},{"name":"mmcblk0p2","type":"part","mountpoint":"/"}]}]}"#;
        assert!(parse_lsblk(j).unwrap().is_empty());
    }

    #[test]
    fn windows_filters() {
        let j = r#"[
          {"Number":0,"FriendlyName":"NVMe SSD","Size":512110190592,"BusType":"NVMe","IsBoot":true,"IsSystem":true,"IsReadOnly":false,"IsOffline":false,"Letters":["C:"]},
          {"Number":2,"FriendlyName":"Generic STORAGE DEVICE","Size":31914983424,"BusType":"USB","IsBoot":false,"IsSystem":false,"IsReadOnly":false,"IsOffline":false,"Letters":"E:"},
          {"Number":3,"FriendlyName":"SDXC Card","Size":63864569856,"BusType":"SD","IsBoot":false,"IsSystem":false,"IsReadOnly":false,"IsOffline":false,"Letters":[]},
          {"Number":4,"FriendlyName":"USB boot","Size":31914983424,"BusType":"USB","IsBoot":true,"IsSystem":false,"IsReadOnly":false,"IsOffline":false,"Letters":null}
        ]"#;
        let d = parse_windows(j).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].device, r"\\.\PhysicalDrive2");
        assert_eq!(d[0].mountpoints, vec!["E:"]);
        assert_eq!(d[1].bus, "sd");
        let single = r#"{"Number":5,"FriendlyName":"X","Size":8000000000,"BusType":"USB","IsBoot":false,"IsSystem":false,"IsReadOnly":false}"#;
        assert_eq!(parse_windows(single).unwrap().len(), 1);
    }

    #[test]
    fn mac_parsing() {
        let list = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>AllDisksAndPartitions</key><array>
  <dict><key>DeviceIdentifier</key><string>disk4</string><key>Size</key><integer>31914983424</integer>
    <key>Partitions</key><array><dict><key>DeviceIdentifier</key><string>disk4s1</string><key>MountPoint</key><string>/Volumes/bootfs</string></dict></array></dict>
</array></dict></plist>"#;
        let l = parse_diskutil_list(list).unwrap();
        assert_eq!(l[0].0, "disk4");
        assert_eq!(l[0].2, vec!["/Volumes/bootfs"]);
        let info = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Internal</key><false/><key>RemovableMedia</key><true/><key>Ejectable</key><true/>
<key>WritableMedia</key><true/><key>MediaName</key><string>Apple SDXC Reader Media</string>
<key>BusProtocol</key><string>USB</string><key>OSInternalMedia</key><false/>
<key>VirtualOrPhysical</key><string>Physical</string></dict></plist>"#;
        let i = parse_diskutil_info(info).unwrap();
        let d = mac_drive(&l[0].0, l[0].1, l[0].2.clone(), &i).unwrap();
        assert_eq!(d.device, "/dev/rdisk4");
        assert!(d.name.starts_with("Apple SDXC Reader Media"));
        let mut sys = i;
        sys.os_internal_media = true;
        assert!(mac_drive("disk4", 31914983424, vec![], &sys).is_none());
    }
}
