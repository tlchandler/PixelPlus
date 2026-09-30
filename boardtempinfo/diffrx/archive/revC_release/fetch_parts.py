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
    "C36693":   "AM26C32IDR quad RS-422 receiver, SOIC-16",
    "C5962":    "74HCT125D quad 3-state buffer, SOIC-14",
    "C201654":  "UA78M05CDCYR 5 V regulator, SOT-223",
    "C99124":   "AOD4184A 40 V 50 A N-channel MOSFET, TO-252",
    "C32677":   "PSM712 asymmetric -7 V / +12 V TVS pair for RS-422 lines, SOT-23",
    "C152077":  "SMDJ15A 3 kW TVS, SMC",
    "C2104":    "BZT52C15 15 V zener, SOD-123",
    "C173405":  "BZT52C6V2 6.2 V zener, SOD-123 (5 V rail clamp)",
    "C4747956": "470 uF 25 V aluminium electrolytic, SMD D8",
    "C208490":  "Bourns MF-R600 PPTC 6 A hold / 12 A trip, 10.2 mm radial",
    "C474882":  "KF301-5.0-3P screw terminal, 3 pole",
    "C475106":  "KF950-9.5-2P screw terminal, 32 A",
    "C385834":  "Ckmtw R-RJ45R08P-A004 RJ45 jack",
    "C2297":    "KT-0805G green LED",
    "C2295":    "KT-0805R red LED (fuse-trip, over-temp and reverse-polarity indicators)",
    "C352820":  "Keystone 3557-2 ATO/ATC blade fuse clip, 30 A - two per fuse",
    "C34565":   "NXP LM75BD I2C temperature sensor, SOIC-8, runs on the 5 V rail",
    "C475499":  "TI LM2903QDRQ1 dual comparator, SOIC-8, AEC-Q100 -40..+125 C",
    "C3195213": "Vishay NTCS0805E3103FHT 10 k B3940 NTC, 0805",
    "C20917":   "AO3400A 30 V N-channel MOSFET, SOT-23 (fan low-side switch)",
    "C8678":    "SS34 40 V 3 A schottky, SMA (fan flyback)",
    "C2876002": "Fuzetec FSMD035-30-1206R PPTC 0.35 A / 30 V, 1206 (fan branch)",
    "C124378":  "Ckmtw B-2100S04P-A110 1x4 2.54 mm pin header (I2C breakout)",
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
