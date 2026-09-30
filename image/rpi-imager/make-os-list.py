#!/usr/bin/env python3
"""Raspberry Pi Imager metadata for PixelPlus images.

1. Describe one built image (sizes + SHA-256 of the compressed and extracted image)::

     make-os-list.py --image pixelplus-1.0.0-trixie-arm64.img.xz --release trixie \\
         --version 1.0.0 --url https://.../pixelplus-1.0.0-trixie-arm64.img.xz \\
         --out os-list-trixie.json

2. Merge fragments into the repository JSON that users paste into Raspberry Pi Imager
   ("App Options -> Content Repository -> Use custom URL", or ``rpi-imager --repo URL``)::

     make-os-list.py --merge os-list-trixie.json os-list-bookworm.json --out pixelplus-imager.json

``init_format`` must match what the base image supports, or Imager's OS customisation
(Wi-Fi, hostname, user, SSH) is silently ignored:
  * Trixie images keep cloud-init (pi-gen ENABLE_CLOUD_INIT=1) -> ``cloudinit-rpi``
  * Bookworm images use Imager's firstrun.sh                     -> ``systemd``
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import lzma
import os
import sys

ICON = "https://raw.githubusercontent.com/tlchandler/PixelPlus/main/image/rpi-imager/icon.svg"
WEBSITE = "https://github.com/tlchandler/PixelPlus"
INIT_FORMAT = {"trixie": "cloudinit-rpi", "bookworm": "systemd"}
# Pi Zero 2 W is tagged pi3-64bit in Raspberry Pi's own device list.
DEVICES = ["pi5-64bit", "pi4-64bit", "pi3-64bit"]
CAPABILITIES = ["i2c", "passwordless_sudo"]

IMAGER_DEVICES = [
    {"name": "Raspberry Pi 5", "tags": ["pi5-64bit"], "description": "Raspberry Pi 5, 500 / 500+, CM5",
     "matching_type": "exclusive",
     "architecture": "armv8", "capabilities": ["i2c"]},
    {"name": "Raspberry Pi 4", "tags": ["pi4-64bit"], "description": "Raspberry Pi 4 Model B, 400, CM4",
     "matching_type": "exclusive",
     "architecture": "armv8", "capabilities": ["i2c"]},
    {"name": "Raspberry Pi Zero 2 W", "tags": ["pi3-64bit"], "description": "Raspberry Pi Zero 2 W",
     "matching_type": "exclusive",
     "architecture": "armv8", "capabilities": ["i2c"]},
    {"name": "Raspberry Pi 3", "tags": ["pi3-64bit"], "description": "Raspberry Pi 3 Model B / B+ / A+",
     "matching_type": "exclusive",
     "architecture": "armv8", "capabilities": ["i2c"]},
]


def describe(path: str, release: str, version: str, url: str, release_date: str | None) -> dict:
    h_dl, h_ex = hashlib.sha256(), hashlib.sha256()
    extract_size = 0
    with open(path, "rb") as f:
        dec = lzma.LZMADecompressor() if path.endswith(".xz") else None
        while True:
            chunk = f.read(4 << 20)
            if not chunk:
                break
            h_dl.update(chunk)
            data = dec.decompress(chunk) if dec else chunk
            h_ex.update(data)
            extract_size += len(data)
    date = release_date or dt.date.fromtimestamp(os.path.getmtime(path)).isoformat()
    label = "recommended" if release == "trixie" else "legacy"
    return {
        "name": f"PixelPlus {version} ({release.capitalize()}, 64-bit)",
        "description": (
            "Pixel controller for xLights shows on difftx / difftxlarge / diffsmart boards. "
            f"Raspberry Pi OS Lite {release.capitalize()} based ({label}). "
            "Wi-Fi and hostname set here are applied on first boot."
        ),
        "icon": ICON,
        "website": WEBSITE,
        "url": url,
        "extract_size": extract_size,
        "extract_sha256": h_ex.hexdigest(),
        "image_download_size": os.path.getsize(path),
        "image_download_sha256": h_dl.hexdigest(),
        "release_date": date,
        "init_format": INIT_FORMAT[release],
        "architecture": "armv8",
        "devices": DEVICES,
        "capabilities": CAPABILITIES,
    }


def merge(fragments: list) -> dict:
    entries = []
    for frag in fragments:
        entries += frag.get("os_list", [frag] if "url" in frag else [])
    # Trixie (recommended) first, newest first.
    entries.sort(key=lambda e: (e.get("init_format") != "cloudinit-rpi", e.get("release_date", "")), reverse=False)
    return {"imager": {"devices": IMAGER_DEVICES}, "os_list": entries}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--image")
    ap.add_argument("--release", choices=sorted(INIT_FORMAT))
    ap.add_argument("--version")
    ap.add_argument("--url")
    ap.add_argument("--release-date")
    ap.add_argument("--merge", nargs="+", metavar="FRAGMENT")
    ap.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    if a.merge:
        frags = []
        for p in a.merge:
            with open(p, encoding="utf-8") as f:
                frags.append(json.load(f))
        doc = merge(frags)
    else:
        if not (a.image and a.release and a.version and a.url):
            ap.error("--image, --release, --version and --url are required")
        doc = {"os_list": [describe(a.image, a.release, a.version, a.url, a.release_date)]}
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
    print(a.out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
