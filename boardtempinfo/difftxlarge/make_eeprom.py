"""
Build the FPP cape EEPROM image for difftxlarge (60 latched DPIPixels outputs on 15 RJ45).

  python make_eeprom.py          -> eeprom/difftxlarge-eeprom.bin (+ the source files under eeprom/difftxlarge/)

Container format is unchanged from ../difftx/make_eeprom.py (FPP docs/EEPROM.txt):
  header  "FPP02", cape name (26), cape version (10), serial (16)
  record  98: location "0" (any)       record 2: tmp/cape-info.tgz (tar.gz of the cape directory)
  end     "0"
Unsigned. FPP signs it in place from Cape Info -> EEPROM Signature (JP1 open, WP low) with a license key
covering >= 60 outputs. Until then FPP treats it as 2 licensed outputs: the latches still work (DPIPixels.cpp
bumps 0 licensed outputs to 2 before its latch check), outputs 3-60 are cut to 50 pixels, and
cape-sensors.json is deleted on load (CapeUtils.cpp) - add the same entries to config/sensors.json by hand.

Output k (0-59) -> jack J(k//4 + 1), port k%4 + 1.  Bank b = k // 20 is latched by latches[b]:
  bank 0 = J1-J5   LE on P1-13 (GPIO27)
  bank 1 = J6-J10  LE on P1-37 (GPIO26)
  bank 2 = J11-J15 LE on P1-22 (GPIO25)
Within a bank, bit i = k % 20 is data line D(i) = GPIO(4 + i). A "sharedOutput" entry's bank is the number
of times its pin has already been used, so the list has to run bank 0, then 1, then 2 (DPIPixels.cpp Init).
"""
import io, os, json, tarfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "eeprom")
CAPE = "difftxlarge"
SRC = os.path.join(OUT, CAPE)
os.makedirs(os.path.join(SRC, "strings"), exist_ok=True)
os.makedirs(os.path.join(SRC, "defaults", "config"), exist_ok=True)

# GPIO4..GPIO23 in DPI bit order -> header pin (DPIPixels.cpp GetDPIPinBitPosition)
DATA_PINS = ["P1-7", "P1-29", "P1-31", "P1-26", "P1-24", "P1-21", "P1-19", "P1-23", "P1-32", "P1-33",
             "P1-8", "P1-10", "P1-36", "P1-11", "P1-12", "P1-35", "P1-38", "P1-40", "P1-15", "P1-16"]
LATCHES = ["P1-13", "P1-37", "P1-22"]          # GPIO27, GPIO26, GPIO25 -> bank 0, 1, 2
N = 60

cape_info = {
    "id": CAPE,
    "version": "1.0",
    "eepromVersion": "0.1",
    "name": "difftxlarge",
    "description": "60-output RS-422 (Falcon differential) pixel cape: 15x RJ45, 4 ports each, 3 latch banks. "
                   "DS3231 RTC, INA226 12 V monitor, 2x LM75B temperature sensors, OLED header.",
    "designer": "DIY",
    "provides": ["strings"],
    # piRTC 2 = "DS1305 / DS1307 / DS3231"; fpprtc exits without hwclock when it is 0 (FPPRTC.cpp).
    # LEDDisplayType 1 = 128x64 SSD1306; harmless when no OLED is fitted (fppoled drops to type 0).
    "defaultSettings": {"piRTC": "2", "LEDDisplayType": "1"},
    "modules": ["rtc-ds1307", "lm75", "ina2xx"],
    # Registered here, not left to fpprtc: on a Pi 5 rtc0 is the SoC's own RTC and fpprtc would never
    # register the DS3231 (FPPRTC.cpp getRTCDev).
    "i2cDevices": ["ds3231 0x68", "lm75b 0x48", "lm75b 0x49", "ina226 0x40"],   # lm75b: 0.125 C steps, not 0.5
    "vendor": {"name": "DIY", "url": "", "image": ""},
}

outputs = [{"pin": p} for p in DATA_PINS]
for bank in (1, 2):
    outputs += [{"sharedOutput": i} for i in range(20)]
assert len(outputs) == N
strings = {
    "name": CAPE,
    "longName": "difftxlarge - 60 outputs, 15x RJ45 differential, 3 latch banks",
    "notes": "Output k -> J(k/4+1) port (k%4+1). RJ45 pairs (+/-) 1/2, 3/6, 5/4, 7/8 = ports 1-4. "
             "Bank 1 = J1-J5 (latch P1-13), bank 2 = J6-J10 (latch P1-37), bank 3 = J11-J15 (latch P1-22).",
    "driver": "DPIPixels",
    "numSerial": 0,
    "outputs": outputs,
    "groups": [{"start": 4 * j + 1, "count": 4, "type": "differential", "portPrefix": f"J{j+1}-", "portStart": 1,
                "label": f"J{j+1} - Differential (bank {j // 5 + 1})"} for j in range(15)],
    "latches": LATCHES,
    # No "pixelLimits": the 2026 DPIPixels interleaves the banks, so every output gets full length
    # (1600 px cap per string); the old "banks" limit would only split that in the UI.
}

# 50 px defaults: the unsigned cap on outputs 3-60, so a fresh board raises no warnings until it is signed.
PX = 50
co_pixel = {"channelOutputs": [{
    "type": "DPIPixels", "subType": CAPE, "pinoutVersion": "1.x", "enabled": 1, "startChannel": 1, "channelCount": -1,
    "outputCount": N, "pixelTiming": 0,
    "outputs": [{"portNumber": p, "protocol": "ws2811", "differentialType": 0,
                 "virtualStrings": [{"description": f"J{p // 4 + 1}-{p % 4 + 1}", "startChannel": p * PX * 3,
                                     "pixelCount": PX, "groupCount": 1, "reverse": 0, "colorOrder": "RGB",
                                     "nullNodes": 0, "endNulls": 0, "zigZag": 0, "brightness": 100, "gamma": "1.0"}]}
                for p in range(N)]}]}

# Only honoured once the EEPROM is signed (CapeUtils deletes tmp/cape-sensors.json otherwise).
# The ina2xx driver assumes a 10 mohm shunt, which is what R_SHUNT on the board is, so no scaling.
def hw(addr, f):
    return f"/sys/bus/i2c/devices/1-00{addr}/hwmon/hwmon0/{f}"
sensors = {"sensors": [
    {"type": "i2c", "label": "Board Temp (drivers): ", "driver": "lm75b", "address": "0x48", "path": hw("48", "temp1_input"),
     "multiplier": 0.001, "valueType": "Temperature", "precision": 1},
    {"type": "i2c", "label": "Board Temp (power): ", "driver": "lm75b", "address": "0x49", "path": hw("49", "temp1_input"),
     "multiplier": 0.001, "valueType": "Temperature", "precision": 1},
    {"type": "i2c", "label": "12V In: ", "driver": "ina226", "address": "0x40", "path": hw("40", "in1_input"),
     "multiplier": 0.001, "postfix": "V", "valueType": "Voltage", "precision": 2},
    {"type": "i2c", "label": "12V Current: ", "driver": "ina226", "address": "0x40", "path": hw("40", "curr1_input"),
     "multiplier": 0.001, "postfix": "A", "valueType": "Current", "precision": 2},
]}

json.dump(cape_info, open(os.path.join(SRC, "cape-info.json"), "w"), indent=4)
json.dump(strings, open(os.path.join(SRC, "strings", f"{CAPE}.json"), "w"), indent=4)
json.dump(co_pixel, open(os.path.join(SRC, "defaults", "config", "co-pixelStrings.json"), "w"), indent=4)
json.dump(sensors, open(os.path.join(SRC, "cape-sensors.json"), "w"), indent=4)

buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:gz") as tar:
    for root, dirs, files in os.walk(SRC):
        for f in sorted(files):
            full = os.path.join(root, f)
            tar.add(full, arcname="./" + os.path.relpath(full, SRC).replace(os.sep, "/"))
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
