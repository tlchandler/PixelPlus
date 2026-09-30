//! Cross-check with independent FAT tools: build an image the way Raspberry Pi OS does
//! (sfdisk MBR, mkfs.vfat FAT32 "bootfs" at 4 MiB), inject pixelplus.txt with our code,
//! read it back with mtools. Skipped when the tools are not installed.

use std::path::Path;
use std::process::Command;

use pixelplus_imager_core::job::customize_image_file;
use pixelplus_imager_core::ImagerSettings;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().map(|o| o.status.success()).unwrap_or(false)
}

fn sh(cmd: &str) -> String {
    let o = Command::new("sh").arg("-c").arg(cmd).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn inject_into_mkfs_vfat_image_readable_by_mtools() {
    if !(have("sfdisk") && have("mkfs.vfat") && have("mcopy")) {
        eprintln!("skipping: needs sfdisk, mkfs.vfat (dosfstools) and mtools");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("pp.img");
    let part = dir.path().join("boot.part");
    let img_s = img.display();
    let part_s = part.display();
    // 4 MiB gap + 64 MiB FAT32 + 8 MiB "rootfs"
    sh(&format!("truncate -s 76M {img_s}"));
    sh(&format!("printf 'label: dos\\nstart=8192, size=131072, type=c\\nstart=139264, type=83\\n' | sfdisk -q {img_s}"));
    sh(&format!("truncate -s 64M {part_s} && mkfs.vfat -F 32 -n bootfs {part_s} >/dev/null"));
    sh(&format!("printf 'hotspot=on\\r\\nwifi_ssid=\\r\\n' > {0}.tpl && mcopy -i {part_s} {0}.tpl ::pixelplus.txt", dir.path().display()));
    sh(&format!("dd if={part_s} of={img_s} bs=512 seek=8192 conv=notrunc status=none"));

    let s = ImagerSettings {
        wifi_ssid: "Café ✨ Wi-Fi".into(),
        wifi_password: "correct horse".into(),
        wifi_country: "GB".into(),
        hostname: "pixelplus-tree".into(),
        ..Default::default()
    };
    customize_image_file(Path::new(&img), &s).unwrap();

    // extract the partition again and read the file with mtools
    sh(&format!("dd if={img_s} of={part_s} bs=512 skip=8192 count=131072 status=none"));
    let text = sh(&format!("mtype -i {part_s} ::pixelplus.txt"));
    assert!(text.contains("wifi_ssid=Café ✨ Wi-Fi\r\n"), "{text}");
    assert!(text.contains("hotspot=on\r\n"));
    assert!(text.contains("hostname=pixelplus-tree\r\n"));
    let dir_listing = sh(&format!("mdir -i {part_s} ::"));
    assert!(dir_listing.to_lowercase().contains("pixelplus.txt"), "{dir_listing}");
    if have("fsck.vfat") {
        sh(&format!("fsck.vfat -n {part_s} >/dev/null"));
    }
}
