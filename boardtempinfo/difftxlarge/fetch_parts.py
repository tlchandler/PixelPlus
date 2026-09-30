"""
Pull every JLCPCB part this board uses into jlc/ (symbols, footprints, 3D models) and sanitise them.

  python fetch_parts.py            # fetch anything missing, then sanitise
  python fetch_parts.py --force    # re-fetch everything

easyeda2kicad copies LCSC's description text verbatim, and some of those descriptions contain raw
newlines inside the quoted string (AOD4184A is one). KiCad then refuses to load the whole library -
a footprint library that fails to parse is reported only as "the current configuration does not
include the footprint library 'jlc'", and a schematic as "Failed to load schematic". So every file
written here gets its quoted strings flattened onto one line.
"""
import os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "jlc", "jlc")

PARTS = {
    "C34923":    "TI AM26C31IDR quad RS-422 driver, SOIC-16",
    "C141311":   "TI SN74AHCT573PWR octal transparent latch, TSSOP-20 (tW 5 ns, for FPP's 26 ns LE pulse)",
    "C50989":    "TI SN74AHCT541PWR octal buffer, TSSOP-20, TTL inputs",
    "C32677":    "PSM712 asymmetric -7 V / +12 V TVS pair for RS-422 lines, SOT-23",
    "C385834":   "Ckmtw R-RJ45R08P-A004 RJ45 jack",
    "C2297":     "KT-0805G green LED (Basic)",
    "C84256":    "NCD0805R1 red LED 0805 (Basic)",
    "C841386":   "TI TPS56637RPAR 4.5-28 V 6 A synchronous buck, VQFN-HR-10",
    "C2962881":  "PSPMAA0805-3R3M-ANP 3.3 uH 17 A sat inductor",
    "C16072":    "AOS AO4407A -30 V -12 A P-MOSFET, SOIC-8 (reverse polarity)",
    "C173427":   "BZT52C15 15 V zener, SOD-123",
    "C152077":   "SMDJ15A 3 kW TVS, SMC",
    "C4747956":  "470 uF 25 V aluminium electrolytic, SMD D8",
    "C352820":   "Keystone 3557-2 ATO blade fuse holder",
    "C474881":   "KF301-5.0-2P screw terminal (12 V in)",
    "C474882":   "KF301-5.0-3P screw terminal (fan, line out)",
    "C2903468":  "10 mohm 1% 2512 metal-alloy shunt",
    "C49851":    "TI INA226AIDGSR current/voltage monitor, VSSOP-10",
    "C107410":   "DS3231MZ+TRL MEMS RTC, SOIC-8",
    "C70377":    "CR2032-BS-6-1 SMT coin-cell holder",
    "C9138":     "BOOMELE 2x20 2.54 mm shrouded box header (Pi ribbon)",
    "C961757":   "PJ-3270-4A 3.5 mm stereo jack (audio in from the Pi)",
    "C668623":   "SHOU HAN TYPE-C 6P USB-C power receptacle (5 V out to the Pi)",
    "C34565":    "NXP LM75BD I2C temperature sensor, SOIC-8",
    "C475499":   "TI LM2903QDRQ1 dual comparator, SOIC-8",
    "C3195213":  "Vishay NTCS0805E3103FHT 10 k NTC, 0805",
    "C20917":    "AO3400A 30 V N-MOSFET, SOT-23 (fan switch)",
    "C8678":     "SS34 40 V 3 A schottky, SMA (fan flyback)",
    "C2876002":  "Fuzetec FSMD035-30-1206R PPTC 0.35 A / 30 V (fan branch)",
    "C2718488":  "1x4 2.54 mm female socket (OLED module)",
    "C6482":     "Microchip AT24C256C-SSHL-T 256 kbit I2C EEPROM, SOIC-8 (FPP cape EEPROM)",
}

def flatten_strings(text):
    out = []; instr = False; esc = False
    for c in text:
        if instr:
            if esc: esc = False
            elif c == "\\": esc = True
            elif c == '"': instr = False
            if c in "\n\r\t" and not esc:
                out.append(" "); continue
        elif c == '"':
            instr = True
        out.append(c)
    return "".join(out)

def sanitise(path):
    t = open(path, encoding="utf-8").read()
    f = flatten_strings(t)
    if f != t:
        open(path, "w", encoding="utf-8", newline="\n").write(f)
        print("  flattened", os.path.relpath(path, HERE))

def main():
    force = "--force" in sys.argv
    for lcsc, what in PARTS.items():
        cmd = [sys.executable, "-m", "easyeda2kicad", "--full", f"--lcsc_id={lcsc}", f"--output={OUT}"]
        if force:
            cmd.append("--overwrite")
        r = subprocess.run(cmd, capture_output=True, text=True)
        tag = "ok" if "Created Kicad footprint" in r.stdout else ("have" if "already exist" in r.stdout + r.stderr else "FAIL")
        print(f"{lcsc:10s} {tag:5s} {what}")
        if tag == "FAIL":
            print(r.stdout, r.stderr)
    sanitise(os.path.join(HERE, "jlc", "jlc.kicad_sym"))
    pretty = os.path.join(HERE, "jlc", "jlc.pretty")
    for f in sorted(os.listdir(pretty)):
        if f.endswith(".kicad_mod"):
            sanitise(os.path.join(pretty, f))

if __name__ == "__main__":
    main()
