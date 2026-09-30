#!/usr/bin/env python3
"""Write the signed-update release index for one channel (F15, docs/ARCHITECTURE.md 12.13).

    packaging/release-index.py --channel stable --version 1.2.3 \
        --base-url https://github.com/OWNER/PixelPlus/releases/download/v1.2.3 \
        --notes-file notes.md --out dist/pixelplus-stable.json dist/pixelplus_1.2.3_*.deb

The index lists one package per architecture with its size and SHA-256. Sign the index and
every .deb with minisign (`minisign -S -s KEY -m FILE`); pixelplusd and the root helper only
accept files signed by a key in packaging/keys/pixelplus-release.pub.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import sys

VERSION_RE = re.compile(r"^[0-9][A-Za-z0-9.+~-]{0,63}$")


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def build(channel, version, base_url, debs, notes=None, proto=(2, 2), format_version=1, date=None):
    if channel not in ("stable", "beta"):
        raise ValueError("channel must be stable or beta")
    if not VERSION_RE.match(version):
        raise ValueError(f"invalid version {version!r}")
    if not base_url.startswith("https://"):
        raise ValueError("the base URL must be https://")
    files = []
    for deb in sorted(debs):
        name = os.path.basename(deb)
        m = re.match(r"^pixelplus_(.+)_(arm64|amd64)\.deb$", name)
        if not m or m.group(1) != version:
            raise ValueError(f"{name} is not a pixelplus {version} package")
        files.append({
            "arch": m.group(2),
            "name": name,
            "size": os.path.getsize(deb),
            "sha256": sha256(deb),
            "url": f"{base_url.rstrip('/')}/{name}",
        })
    if not files:
        raise ValueError("no packages")
    return {
        "v": 1,
        "channel": channel,
        "version": version,
        "date": date or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "notes": notes,
        "protoMin": proto[0],
        "protoMax": proto[1],
        "formatVersion": format_version,
        "files": files,
    }


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--channel", required=True, choices=["stable", "beta"])
    ap.add_argument("--version", required=True)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--notes-file")
    ap.add_argument("--proto-min", type=int, default=2)
    ap.add_argument("--proto-max", type=int, default=2)
    ap.add_argument("--format-version", type=int, default=1)
    ap.add_argument("--out", required=True)
    ap.add_argument("debs", nargs="+")
    a = ap.parse_args(argv)
    notes = None
    if a.notes_file and os.path.exists(a.notes_file):
        with open(a.notes_file, encoding="utf-8") as f:
            notes = f.read().strip()[:20000] or None
    idx = build(a.channel, a.version, a.base_url, a.debs, notes, (a.proto_min, a.proto_max), a.format_version)
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(idx, f, indent=2)
        f.write("\n")
    print(f"wrote {a.out}: {a.channel} {a.version} ({', '.join(x['arch'] for x in idx['files'])})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
