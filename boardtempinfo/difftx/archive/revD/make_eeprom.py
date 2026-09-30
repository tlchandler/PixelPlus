"""
Build the FPP cape EEPROM image for this board.

  python make_eeprom.py          -> eeprom/difftx-eeprom.bin (+ the source files under eeprom/difftx/)

Format follows FPP's docs/EEPROM.txt and www/fppEEPROM.php (generateData):
  header  pack('a6a26a10a16'): "FPP02", cape name, cape version, cape serial
  record  pack('a6a2', length, code) ...
          code 98: 2-byte location ("0" = any: physical EEPROM or virtual file)
          code  2: 64-byte filename "tmp/cape-info.tgz" then a tar.gz of the cape directory
  end     pack('a6', "0")
Unsigned; FPP signs it in place through its web UI (voucher/key) and writes the signed
image back to /sys/bus/i2c/devices/1-0050/eeprom, which is why WP must be low then.
"""
import io, os, json, struct, tarfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "eeprom")
CAPE = "difftx"
SRC = os.path.join(OUT, CAPE)
os.makedirs(os.path.join(SRC, "strings"), exist_ok=True)
os.makedirs(os.path.join(SRC, "defaults", "config"), exist_ok=True)

cape_info = {
    "id": CAPE,
    "version": "1.0",
    "eepromVersion": "0.1",
    "name": "difftx RS-422 4-port",
    "provides": ["strings"],
    "description": "4-port RS-422 (Falcon differential receiver) pixel driver pHAT for Raspberry Pi Zero 2 W, rev D",
    "designer": "DIY",
    "vendor": {"name": "DIY", "url": "", "image": ""},
}
# outputs[] order = FPP string/port number. Pin names are the header pins FPP's
# DPIPixels driver accepts (GetDPIPinBitPosition). Order below = RJ45 port order for the rev D board.
strings = {
    "name": CAPE,
    "longName": "difftx 4-port RS-422 differential (Falcon pinout)",
    "notes": "One RJ45: port 1 = pins 1/2, port 2 = 3/6, port 3 = 4/5, port 4 = 7/8. Standard (dumb) differential receivers.",
    "driver": "DPIPixels",
    "numSerial": 0,
    "outputs": [
        {"pin": "P1-29"},   # port 1: GPIO5  DPI_D1 -> U2 1A -> 1Y/1Z -> RJ45 1/2   (rev D wiring)
        {"pin": "P1-31"},   # port 2: GPIO6  DPI_D2 -> U2 3A -> 3Y/3Z -> RJ45 3/6
        {"pin": "P1-26"},   # port 3: GPIO7  DPI_D3 -> U2 2A -> 2Y/2Z -> RJ45 4/5
        {"pin": "P1-7"},    # port 4: GPIO4  DPI_D0 -> U2 4A -> 4Y/4Z -> RJ45 7/8
    ],
    "groups": [{"start": 1, "count": 4, "type": "differential", "label": "Differential Port #1 (RJ45)"}],
}
co_pixel = {"channelOutputs": [{
    "type": "DPIPixels", "subType": CAPE, "pinoutVersion": "1.x", "enabled": 1, "startChannel": 1, "channelCount": -1,
    "outputCount": 4, "pixelTiming": 0,
    "outputs": [{"portNumber": p, "protocol": "ws2811",
                 "virtualStrings": [{"description": f"Port {p+1}", "startChannel": p * 1200, "pixelCount": 400, "groupCount": 1,
                                     "reverse": 0, "colorOrder": "RGB", "nullNodes": 0, "endNulls": 0, "zigZag": 0,
                                     "brightness": 100, "gamma": "1.0"}]} for p in range(4)]}]}
json.dump(cape_info, open(os.path.join(SRC, "cape-info.json"), "w"), indent=4)
json.dump(strings, open(os.path.join(SRC, "strings", f"{CAPE}.json"), "w"), indent=4)
json.dump(co_pixel, open(os.path.join(SRC, "defaults", "config", "co-pixelStrings.json"), "w"), indent=4)

# tar.gz of the cape directory, paths relative to it like "(cd dir && tar cvzf - ./)"
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:gz") as tar:
    for root, dirs, files in os.walk(SRC):
        for f in sorted(files):
            full = os.path.join(root, f)
            arc = "./" + os.path.relpath(full, SRC).replace(os.sep, "/")
            tar.add(full, arcname=arc)
tgz = buf.getvalue()

def a(s, n):
    b = s.encode() if isinstance(s, str) else s
    return b[:n].ljust(n, b"\0")

serial = time.strftime("%Y%m%d%H%M%S")
data = a("FPP02", 6) + a(CAPE, 26) + a("1.0", 10) + a(serial, 16)
data += a("2", 6) + a("98", 2) + a("0", 2)                                   # location: any
data += a(str(len(tgz)), 6) + a("2", 2) + a("tmp/cape-info.tgz", 64) + tgz   # tar.gz payload
data += a("0", 6)                                                            # end of records
assert len(data) <= 32768, "does not fit a 24C256"
out = os.path.join(OUT, f"{CAPE}-eeprom.bin")
open(out, "wb").write(data)
print(f"wrote {out}: {len(data)} bytes (serial {serial}); tgz {len(tgz)} bytes")
