"""
Schematic generator for diffrx rev A: the receiver twin of the difftx FPP pHAT.

  python gen_sch.py

Outputs: diffrx.kicad_sch, diffrx.kicad_pro, diffrx.kicad_sym, fp-lib-table, sym-lib-table

What the board is (see VERIFICATION.md for the sources every number came from):
  One cat5 in (J14, Falcon differential pinout, the same pin pairs the difftx board drives), four WS2811
  pixel data outputs, twelve 3-pole screw terminals (4 data+power, 8 power-injection only), and a 12 V 30 A
  power distribution bus with a resettable fuse per output, so the board replaces the marine fuse panel.

  J14   RJ45 8P8C.  pins 1/2 = port 1 (+/-), 3/6 = port 2, 4/5 = port 3, 7/8 = port 4   (matches difftx)
  RT1-4 120 R across each pair; RB1-8 1 k fail-safe bias (A low, B high) so a dead or unplugged
        transmitter leaves the receiver output LOW, which is the WS2811 idle state
  D3-D6 one PSM712 asymmetric TVS pair per cat5 pair (+12 V / -7 V stand-off, keeps the
        receiver's common-mode window intact); D7 clamps the 5 V rail
  U1    AM26C32IDR quad RS-422 receiver.  A = "+", B = "-", Y = data.  G high / ~G low = enabled
  RS1-4 470 R series at each data output (damping plus fault limiting); LED1-4 blink with data
  U3    UA78M05 5 V linear regulator, fed from +12 V through R3 10 R (surge) - about 50 mA of load

  J1    12 V 30 A input, 9.5 mm screw terminal.  UPPER terminal = +12 V, LOWER = GND (silk marked)
  Q1-Q4 four AOD4184A in parallel in the ground return = reverse-polarity block (about 2 mohm total).
        Gate from +12 V through R1 10 k, clamped by D1 15 V, pulled down by R2 100 k.
        The body diodes conduct in the normal direction, so the board powers up before the gates charge.
  D2    SMDJ15A 3 kW TVS across the protected rails (NOT across the input: a unidirectional TVS ahead of
        the FETs would be a dead short on a reversed supply)
  C1    470 uF 25 V bulk on the 12 V bus
  F1-12 MF-R400 PPTC, 4 A hold / 8 A trip, 30 V, one per output terminal
  J2-13 KF301-5.0-3P outputs, silk-marked + / D / -.  J2/J5/J8/J11 are the four data ports; the other
        eight are power injection only and their middle (data) pole is deliberately NOT connected here.
"""
import os, re, uuid, json

HERE = os.path.dirname(os.path.abspath(__file__))
KISYM = r"C:\Program Files\KiCad\10.0\share\kicad\symbols"
PROJECT = "diffrx"

def U():
    return str(uuid.uuid4())

def block_at(t, i):
    depth = 0; j = i
    while True:
        c = t[j]
        if c == '(':
            depth += 1
        elif c == ')':
            depth -= 1
            if depth == 0:
                return t[i:j + 1]
        j += 1

def lib_block(text, name):
    i = text.find(f'(symbol "{name}"\n')
    if i < 0:
        i = text.find(f'(symbol "{name}"')
    assert i >= 0, name
    return block_at(text, i)

def lib_symbol(libfile, name):
    t = open(os.path.join(KISYM, libfile), encoding="utf-8").read()
    b = lib_block(t, name)
    ext = re.findall(r'\(extends "([^"]*)"', b)
    if ext:
        parent = lib_block(t, ext[0])
        units = re.findall(r'\n\t\t\(symbol "' + re.escape(ext[0]) + r'_(\d+_\d+)"', parent)
        unit_blocks = ""
        for u in units:
            ub = block_at(parent, parent.find(f'(symbol "{ext[0]}_{u}"'))
            unit_blocks += "\n\t\t" + ub.replace(f'(symbol "{ext[0]}_{u}"', f'(symbol "{name}_{u}"', 1)
        b = re.sub(r'\n\t\t\(extends "[^"]*"\)', '', b)
        pn = re.search(r'\n\t\t\(pin_names[^\n]*(?:\n\t\t\t[^\n]*)*\n\t\t\)', parent)
        pnum = re.search(r'\n\t\t\(pin_numbers[^\n]*(?:\n\t\t\t[^\n]*)*\n\t\t\)', parent)
        extra = (pn.group(0) if pn else "") + (pnum.group(0) if pnum else "")
        b = b.replace(f'(symbol "{name}"', f'(symbol "{name}"' + extra, 1)
        b = b[:b.rstrip().rindex(')')] + unit_blocks + "\n\t)"
    return b

def rename_symbol(block, oldname, newname):
    block = block.replace(f'(symbol "{oldname}"', f'(symbol "{PROJECT}:{newname}"', 1)
    block = re.sub(r'\(symbol "' + re.escape(oldname) + r'_(\d+_\d+)"', lambda m: f'(symbol "{newname}_{m.group(1)}"', block)
    return block

def set_prop(block, prop, value):
    return re.sub(r'\(property "' + prop + r'" "[^"]*"', f'(property "{prop}" "{value}"', block, count=1)

PASSIVE = re.compile(r'\(pin (input|output|bidirectional|unspecified|power_in|power_out|tri_state|open_collector|open_emitter|free) ')
def passivate(block):
    """JLC/EasyEDA symbols give pins arbitrary electrical types; ERC only makes sense if they are passive."""
    return PASSIVE.sub('(pin passive ', block)

def flatten_strings(block):
    """EasyEDA's ki_description holds a multi-line datasheet blurb; a raw newline inside a quoted
    string makes KiCad fail the whole file with nothing but "Failed to load schematic"."""
    out = []; instr = False; esc = False
    for c in block:
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

PIN_RE = re.compile(r'\(pin (\w+) \w+\s*\(at ([-\d.]+) ([-\d.]+) (\d+)\)\s*\(length ([\d.]+)\).*?\(name "([^"]*)".*?\(number "([^"]*)"', re.S)
def pins_of(block):
    out = {}
    for m in PIN_RE.finditer(block):
        out.setdefault(m.group(7), (float(m.group(2)), float(m.group(3)), int(m.group(4)), m.group(6), m.group(1)))
    return out

def prop(name, value, x, y, hide=False, justify=None):
    x = round(x, 4); y = round(y, 4)
    j = f"\n\t\t\t\t(justify {justify})" if justify else ""
    h = "\n\t\t\t(hide yes)" if hide else ""
    return (f'\t\t(property "{name}" "{value}"\n\t\t\t(at {x:g} {y:g} 0)\n\t\t\t(show_name no)\n\t\t\t(do_not_autoplace no){h}'
            f'\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t){j}\n\t\t\t)\n\t\t)\n')

# ----------------------------------------------------------------------
# library symbols
# ----------------------------------------------------------------------
jlc_lib = open(os.path.join(HERE, "jlc", "jlc.kicad_sym"), encoding="utf-8").read()

def jlc_symbol(easyeda_name, newname, value=None):
    b = flatten_strings(passivate(rename_symbol(lib_block(jlc_lib, easyeda_name), easyeda_name, newname)))
    if value:
        b = set_prop(b, "Value", value)
    return b

sym_rx   = rename_symbol(lib_symbol("Interface.kicad_sym", "AM26LV32xD"), "AM26LV32xD", "AM26C32")
sym_rx   = set_prop(sym_rx, "Value", "AM26C32IDR")
sym_rx   = set_prop(sym_rx, "Footprint", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL")
sym_reg  = jlc_symbol("UA78M05CDCYR", "UA78M05", "UA78M05CDCYR")
sym_fet  = jlc_symbol("AOD4184A", "AOD4184A")
sym_esd  = jlc_symbol("PSM712-LF-T7", "PSM712", "PSM712")
sym_tvs  = jlc_symbol("SMDJ15A", "SMDJ15A")
sym_zen  = jlc_symbol("BZT52C15_C2104", "BZT52C15", "BZT52C15")
sym_led  = jlc_symbol("0805G", "LED", "LED")
sym_cp   = jlc_symbol("RST470UF25V032", "CP", "470uF")
sym_rj   = jlc_symbol("R-RJ45R08P-A004", "RJ45_8P8C", "R-RJ45R08P-A004")
sym_fuse = rename_symbol(lib_symbol("Device.kicad_sym", "Polyfuse"), "Polyfuse", "Polyfuse")
sym_mainfuse = rename_symbol(lib_symbol("Device.kicad_sym", "Fuse"), "Fuse", "Fuse")
sym_t3   = rename_symbol(lib_symbol("Connector.kicad_sym", "Screw_Terminal_01x03"), "Screw_Terminal_01x03", "Screw_Terminal_01x03")
sym_t2   = rename_symbol(lib_symbol("Connector.kicad_sym", "Screw_Terminal_01x02"), "Screw_Terminal_01x02", "Screw_Terminal_01x02")
sym_r    = rename_symbol(lib_symbol("Device.kicad_sym", "R"), "R", "R")
sym_c    = rename_symbol(lib_symbol("Device.kicad_sym", "C"), "C", "C")
sym_hole = rename_symbol(lib_symbol("Mechanical.kicad_sym", "MountingHole"), "MountingHole", "MountingHole")

pw = open(os.path.join(KISYM, "power.kicad_sym"), encoding="utf-8").read()
def power_sym(name):
    return lib_block(pw, name).replace(f'(symbol "{name}"', f'(symbol "power:{name}"', 1)
sym_gnd, sym_5v, sym_12v, sym_flag = (power_sym(n) for n in ("GND", "+5V", "+12V", "PWR_FLAG"))

LIB_SYMBOLS = "\t(lib_symbols\n" + "\n".join(
    [sym_rx, sym_reg, sym_fet, sym_esd, sym_tvs, sym_zen, sym_led, sym_cp, sym_rj,
     sym_fuse, sym_mainfuse, sym_t3, sym_t2, sym_r, sym_c, sym_hole, sym_gnd, sym_5v, sym_12v, sym_flag]) + "\n\t)"

# ----------------------------------------------------------------------
# schematic
# ----------------------------------------------------------------------
ROOT_UUID = U()
items = []
pwr_count = [0]

def sym_instance(lib_id, ref, value, footprint, x, y, pins, extra_props=None, datasheet="", descr=""):
    lines = [f'\t(symbol\n\t\t(lib_id "{lib_id}")\n\t\t(at {x:g} {y:g} 0)\n\t\t(unit 1)\n\t\t(body_style 1)\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(in_pos_files yes)\n\t\t(dnp no)\n\t\t(uuid "{U()}")']
    lines.append(prop("Reference", ref, x, y - 2.54, hide=ref.startswith("#")))
    lines.append(prop("Value", value, x, y + 2.54, hide=(value == "PWR_FLAG")))
    lines.append(prop("Footprint", footprint, x, y, hide=True))
    lines.append(prop("Datasheet", datasheet, x, y, hide=True))
    lines.append(prop("Description", descr, x, y, hide=True))
    for k, v in (extra_props or {}).items():
        lines.append(prop(k, v, x, y, hide=True))
    for pn in pins:
        lines.append(f'\t\t(pin "{pn}"\n\t\t\t(uuid "{U()}")\n\t\t)')
    lines.append(f'\t\t(instances\n\t\t\t(project "{PROJECT}"\n\t\t\t\t(path "/{ROOT_UUID}"\n\t\t\t\t\t(reference "{ref}")\n\t\t\t\t\t(unit 1)\n\t\t\t\t)\n\t\t\t)\n\t\t)\n\t)')
    items.append("\n".join(lines))

def wire(x1, y1, x2, y2):
    items.append(f'\t(wire\n\t\t(pts\n\t\t\t(xy {x1:g} {y1:g}) (xy {x2:g} {y2:g})\n\t\t)\n\t\t(stroke\n\t\t\t(width 0)\n\t\t\t(type default)\n\t\t)\n\t\t(uuid "{U()}")\n\t)')
def label(text, x, y, angle, justify):
    items.append(f'\t(label "{text}"\n\t\t(at {x:g} {y:g} {angle})\n\t\t(effects\n\t\t\t(font\n\t\t\t\t(size 1.27 1.27)\n\t\t\t)\n\t\t\t(justify {justify})\n\t\t)\n\t\t(uuid "{U()}")\n\t)')
def no_connect(x, y):
    items.append(f'\t(no_connect\n\t\t(at {x:g} {y:g})\n\t\t(uuid "{U()}")\n\t)')
def text(s, x, y, size=2.0):
    items.append(f'\t(text "{s}"\n\t\t(exclude_from_sim no)\n\t\t(at {x:g} {y:g} 0)\n\t\t(effects\n\t\t\t(font\n\t\t\t\t(size {size} {size})\n\t\t\t)\n\t\t\t(justify left bottom)\n\t\t)\n\t\t(uuid "{U()}")\n\t)')
def power(kind, x, y):
    pwr_count[0] += 1
    sym_instance(f"power:{kind}", f"#PWR{pwr_count[0]:02d}", kind, "", x, y, ["1"], descr=f"Power symbol {kind}")
def pwr_flag(x, y):
    pwr_count[0] += 1
    sym_instance("power:PWR_FLAG", f"#FLG{pwr_count[0]:02d}", "PWR_FLAG", "", x, y, ["1"], descr="Power flag")

OUT = {0: (-1, 0), 180: (1, 0), 90: (0, 1), 270: (0, -1)}

def place(lib_id, ref, value, footprint, x, y, block, conn, datasheet="", descr="", lcsc="", stub=2.54):
    pins = pins_of(block)
    sym_instance(lib_id, ref, value, footprint, x, y, list(pins.keys()), datasheet=datasheet, descr=descr,
                 extra_props={"LCSC": lcsc} if lcsc else None)
    done = set()
    for pn, (px, py, ang, name, ptype) in pins.items():
        cx, cy = x + px, y - py
        c = conn.get(pn, ("NC",))
        key = (round(cx, 3), round(cy, 3))
        if key in done:
            continue
        done.add(key)
        dx, dy = OUT[ang]
        ex, ey = cx + dx * stub, cy + dy * stub
        if c[0] == "NC":
            no_connect(cx, cy); continue
        wire(cx, cy, ex, ey)
        if c[0] == "label":
            if dx < 0: label(c[1], ex, ey, 180, "right bottom")
            elif dx > 0: label(c[1], ex, ey, 0, "left bottom")
            elif dy < 0: label(c[1], ex, ey, 90, "left bottom")
            else: label(c[1], ex, ey, 270, "right bottom")
        elif c[0] in ("GND", "+5V", "+12V"):
            power(c[0], ex, ey)
            if len(c) > 1 and c[1] == "flag":
                pwr_flag(ex, ey)

R0603 = "Resistor_SMD:R_0603_1608Metric"
R1206 = "Resistor_SMD:R_1206_3216Metric"
C0805 = "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder"
C1206 = "Capacitor_SMD:C_1206_3216Metric"
FP_LED = "jlc:LED0805-R-RD"

def res(ref, value, x, y, a, b, descr, lcsc, fp=R0603):
    place(f"{PROJECT}:R", ref, value, fp, x, y, sym_r, {"1": a, "2": b}, descr=descr, lcsc=lcsc)
def cap(ref, value, x, y, a, b, descr, lcsc, fp=C0805):
    place(f"{PROJECT}:C", ref, value, fp, x, y, sym_c, {"1": a, "2": b}, descr=descr, lcsc=lcsc)

# ---- cat5 input, termination, fail-safe bias, ESD ---------------------------------------------
PAIR = {1: ("P1P", "P1N"), 2: ("P2P", "P2N"), 3: ("P3P", "P3N"), 4: ("P4P", "P4N")}
place(f"{PROJECT}:RJ45_8P8C", "J14", "R-RJ45R08P-A004", "jlc:RJ45-TH_R-RJ45R08P-A004", 40.64, 76.2, sym_rj, {
    "1": ("label", "P1P"), "2": ("label", "P1N"), "3": ("label", "P2P"), "4": ("label", "P3P"),
    "5": ("label", "P3N"), "6": ("label", "P2N"), "7": ("label", "P4P"), "8": ("label", "P4N"),
}, datasheet="https://www.lcsc.com/datasheet/C385834.pdf",
   descr="RJ45 8P8C jack, right angle, unshielded. Falcon differential pinout: port1 = 1/2, port2 = 3/6, port3 = 4/5, port4 = 7/8",
   lcsc="C385834")

for i in range(1, 5):
    p, n = PAIR[i]
    y = 40.64 + (i - 1) * 25.4
    res(f"RT{i}", "120R", 88.9, y, ("label", p), ("label", n),
        "RS-422 line termination across the pair (Cat5 is 100 ohm; 120 ohm keeps the fail-safe bias above the 200 mV receiver threshold)",
        "C17909", fp=R1206)
    res(f"RB{2*i-1}", "1k", 109.22, y, ("label", p), ("GND",),
        "Fail-safe bias: pulls A low so the receiver output idles LOW (WS2811 idle) when the cat5 is unplugged or the transmitter is off", "C21190")
    res(f"RB{2*i}", "1k", 132.08, y, ("label", n), ("+5V",),
        "Fail-safe bias: pulls B high (see the matching RB on the A side)", "C21190")

# One PSM712 per pair. Unlike a 5 V rail-clamp array (an SRV05-4 and friends), this part stands off
# +12 V / -7 V per line to ground, which is exactly the AM26C32's common-mode window - so it protects
# the pair without throwing away the +/-7 V of ground offset that is the whole reason for using RS-422
# on a cable that carries no ground wire. It also needs no connection to the 5 V rail.
for k, (ref, p_, n_) in enumerate([("D3", "P1P", "P1N"), ("D4", "P2P", "P2N"),
                                   ("D5", "P3P", "P3N"), ("D6", "P4P", "P4N")]):
    place(f"{PROJECT}:PSM712", ref, "PSM712", "jlc:SOT-23-3_L3.0-W1.7-P0.95-LS2.9-BR",
          63.5, 160.02 + k * 20.32, sym_esd,
          {"1": ("label", p_), "2": ("label", n_), "3": ("GND",)},
          datasheet="https://protekdevices.com/wp-content/uploads/datasheets/psm712.pdf",
          descr=f"Asymmetric TVS pair on the {p_[:2]} cat5 conductors (SOT-23: 1 and 2 = I/O, 3 = GND; "
                f"7 V stand-off negative, 12 V positive, per ProTek 05094)", lcsc="C32677")

# ---- receiver ---------------------------------------------------------------------------------
# Which receiver channel takes which cat5 pair is a layout choice, not an electrical one: the pairs are
# assigned to whichever channel sits nearest them on the board, and the port number follows the pair.
place(f"{PROJECT}:AM26C32", "U1", "AM26C32IDR", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", 165.1, 88.9, sym_rx, {
    "2": ("label", "P4P"), "1": ("label", "P4N"), "3": ("label", "RXD4"),
    "6": ("label", "P1P"), "7": ("label", "P1N"), "5": ("label", "RXD1"),
    "10": ("label", "P2P"), "9": ("label", "P2N"), "11": ("label", "RXD2"),
    "14": ("label", "P3P"), "15": ("label", "P3N"), "13": ("label", "RXD3"),
    "4": ("+5V",), "12": ("GND",), "8": ("GND",), "16": ("+5V",),
}, datasheet="https://www.ti.com/lit/ds/symlink/am26c32.pdf",
   descr="Quad RS-422 differential line receiver, SOIC-16. A = '+', B = '-', Y = H when A-B > 200 mV. G high and ~G low = enabled",
   lcsc="C36693")
cap("C6", "100nF", 190.5, 66.04, ("+5V",), ("GND",), "Ceramic bypass 100 nF 50 V X7R at the AM26C32 VCC", "C49678")

# ---- series resistors and activity LEDs -------------------------------------------------------
# No output buffer: the AM26C32 drives each port directly through 470 R. See RS description below for
# the fault arithmetic; 470 R into the few hundred pF of a pigtail is a ~100 ns edge against a 1.25 us
# WS2811 bit, and D7 clamps the 5 V rail against back-feed.
for i in range(1, 5):
    y = 111.76 + (i - 1) * 20.32
    res(f"RS{i}", "470R", 289.56, y, ("label", f"RXD{i}"), ("label", f"DATA{i}"),
        "Series damping and fault limiting. TI SLLS104M rates the AM26C32 at +/-6 mA recommended and "
        "+/-25 mA absolute maximum: 470 R holds a data core shorted to ground to 10.6 mA and one shorted "
        "to the +12 V screw beside it to 13.4 mA, which the 5 V rail's own load absorbs", "C23179")
    place(f"{PROJECT}:LED", f"LED{i}", "green", FP_LED, 317.5, y, sym_led,
          {"1": ("label", f"RXD{i}"), "2": ("label", f"LK{i}")},
          descr=f"Port {i} data activity LED (lit while data is clocking out)", lcsc="C2297")
    res(f"RL{i}", "1k", 342.9, y, ("label", f"LK{i}"), ("GND",), f"Port {i} activity LED series resistor", "C21190")

place(f"{PROJECT}:LED", "LED5", "yellow", FP_LED, 317.5, 195.58, sym_led,
      {"1": ("+5V",), "2": ("label", "LK5")}, descr="5 V rail power indicator", lcsc="C2296")
res("RL5", "1k", 342.9, 195.58, ("label", "LK5"), ("GND",), "Power LED series resistor", "C21190")

# ---- 12 V input, reverse-polarity block, TVS, bulk, 5 V regulator ------------------------------
place(f"{PROJECT}:Screw_Terminal_01x02", "J1", "12V 30A IN", "jlc:CONN-TH_P9.50_KF950-9.5-2P", 38.1, 264.16, sym_t2,
      {"1": ("label", "GNDIN"), "2": ("label", "12VIN")},
      descr="12 V 30 A input, 9.5 mm screw terminal, 10-22 AWG, 32 A. Pin 2 (upper on the board) = +12 V, pin 1 (lower) = supply negative",
      lcsc="C475106")
pwr_flag(38.1, 297.18); wire(38.1, 297.18, 38.1, 289.56); label("GNDIN", 38.1, 289.56, 90, "left bottom")
# Nothing stood between the input terminal and the bus before: a TVS or a bulk capacitor fails SHORT,
# and a shorted part across an unfused 30 A supply is a fire. F0 is a bolt-down MIDI/AMI automotive fuse
# on two M5 studs, so every tap off the 12 V bus - the twelve outputs, the TVS, the bulk cap and the
# regulator feed - sits behind it. This is what lets the board replace the fuse panel outright.
place(f"{PROJECT}:Fuse", "F0", "MIDI 30A", f"{PROJECT}:Fuse_MIDI_AMI_M5_30mm", 12.7, 264.16, sym_mainfuse,
      {"1": ("label", "12VIN"), "2": ("+12V", "flag")},
      descr="Main input fuse: bolt-down MIDI (AMI) automotive fuse, 30 A, on two M5 studs. Not fitted by "
            "JLCPCB - bolt it on after assembly")

for i in range(1, 5):
    place(f"{PROJECT}:AOD4184A", f"Q{i}", "AOD4184A", "jlc:TO-252-2_L6.6-W6.1-P4.57-LS9.9-TL-CW",
          88.9 + (i - 1) * 30.48, 271.78, sym_fet,
          {"1": ("label", "VG"), "2": ("label", "GNDIN"), "3": ("GND", "flag") if i == 1 else ("GND",)},
          datasheet="https://www.lcsc.com/datasheet/C99124.pdf",
          descr="Reverse-polarity block, 40 V 50 A N-channel, four in parallel in the ground return (about 2 mohm, under 2 W at 30 A)",
          lcsc="C99124")
res("R1", "10k", 68.58, 236.22, ("+12V",), ("label", "VG"), "Gate pull-up for the reverse-polarity FETs", "C25804")
res("R2", "100k", 88.9, 241.3, ("label", "VG"), ("GND",), "Gate pull-down so the FETs turn off when the supply is removed", "C25803")
place(f"{PROJECT}:BZT52C15", "D1", "BZT52C15", "jlc:SOD-123_L2.8-W1.8-LS3.7-RD", 109.22, 241.3, sym_zen,
      {"1": ("label", "VG"), "2": ("GND",)},
      datasheet="https://www.lcsc.com/datasheet/C2104.pdf",
      descr="15 V zener clamping Vgs of the reverse-polarity FETs (pin 1 = cathode)", lcsc="C2104")
place(f"{PROJECT}:SMDJ15A", "D2", "SMDJ15A", "jlc:SMC_L6.9-W5.9-LS7.9-RD", 165.1, 264.16, sym_tvs,
      {"1": ("+12V",), "2": ("GND",)},
      datasheet="https://www.lcsc.com/datasheet/C152077.pdf",
      descr="3 kW 15 V TVS across the protected rails. Deliberately downstream of the FETs: ahead of them it would short a reversed supply",
      lcsc="C152077")
place(f"{PROJECT}:CP", "C1", "470uF 25V", "jlc:CAP-SMD_BD8.0-L8.3-W8.3-FD", 190.5, 264.16, sym_cp,
      {"1": ("+12V",), "2": ("GND",)},
      datasheet="https://www.lcsc.com/datasheet/C4747956.pdf",
      descr="Bulk capacitor on the 12 V bus (pin 1 = +). Damps the inductance of the run back to the supply", lcsc="C4747956")

res("R3", "10R", 215.9, 236.22, ("+12V",), ("label", "VREG"), "Feeds the regulator through a resistor so a clamped surge cannot slam its 25 V input", "C22859")
cap("C2", "10uF", 241.3, 248.92, ("label", "VREG"), ("GND",), "Regulator input bulk, 10 uF 50 V X5R 1206", "C13585", fp=C1206)
cap("C3", "100nF", 261.62, 248.92, ("label", "VREG"), ("GND",), "Regulator input bypass 100 nF 50 V", "C49678")
place(f"{PROJECT}:UA78M05", "U3", "UA78M05CDCYR", "jlc:SOT-223_L6.5-W3.5-P2.30-LS7.0-BR", 289.56, 236.22, sym_reg,
      {"1": ("label", "VREG"), "2": ("GND",), "3": ("+5V", "flag"), "4": ("GND",)},
      datasheet="https://www.ti.com/lit/ds/symlink/ua78m33c.pdf",
      descr="5 V 500 mA linear regulator, SOT-223 (1 = IN, 2 and tab = GND, 3 = OUT). Load is about 50 mA, so it dissipates under 0.4 W",
      lcsc="C201654")
cap("C4", "22uF", 330.2, 248.92, ("+5V",), ("GND",), "Regulator output 22 uF 25 V X5R", "C45783")
cap("C5", "100nF", 350.52, 248.92, ("+5V",), ("GND",), "Regulator output bypass 100 nF", "C49678")
# A pixel data core shorted to the +12 V screw 5 mm away on the same terminal injects current back into
# the 5 V rail through the receiver's output clamp, and a linear regulator cannot sink it. The rail's own
# ~40 mA load absorbs up to three such faults; this zener catches the rest, well under the AM26C32's 7 V
# absolute-maximum VCC.
place(f"{PROJECT}:BZT52C15", "D7", "BZT52C6V2", "jlc:SOD-123_L2.8-W1.8-LS3.7-RD", 370.84, 248.92, sym_zen,
      {"1": ("+5V",), "2": ("GND",)},
      datasheet="https://www.lcsc.com/datasheet/C173405.pdf",
      descr="6.2 V zener clamping the 5 V rail against back-feed from a mis-wired output (pin 1 = cathode)",
      lcsc="C173405")

# ---- fused outputs ----------------------------------------------------------------------------
# J2/J5/J8/J11 carry data; the other eight are power injection and their middle pole is a no-connect.
PORTS = [("P1 DATA", 1), ("P1 INJ A", 0), ("P1 INJ B", 0), ("P2 DATA", 2), ("P2 INJ A", 0), ("P2 INJ B", 0),
         ("P3 DATA", 3), ("P3 INJ A", 0), ("P3 INJ B", 0), ("P4 DATA", 4), ("P4 INJ A", 0), ("P4 INJ B", 0)]
for k, (name, data) in enumerate(PORTS):
    y = 38.1 + k * 30.48
    place(f"{PROJECT}:Polyfuse", f"F{k+1}", "6A/12A 30V", "jlc:FUSE-TH_L19.1-W3.0-P10.20-D1.2-S2.0", 431.8, y, sym_fuse,
          {"1": ("+12V",), "2": ("label", f"V{k+1}")},
          datasheet="https://www.lcsc.com/datasheet/C208490.pdf",
          descr=f"Resettable fuse for {name}: Bourns MF-R600, 6 A hold / 12 A trip at 23 C (3.66 A hold at "
                f"60 C), 30 V, 40 A interrupt. Sized to an 18 AWG pigtail core",
          lcsc="C208490")
    mid = ("label", f"DATA{data}") if data else ("NC",)
    # Both rows read + / D / - left to right on the board. The top row is fitted rotated 180 degrees so its
    # poles come out reversed, which is why its fused +12 V lands on pole 3 and its ground on pole 1.
    poles = ({"1": ("GND",), "2": mid, "3": ("label", f"V{k+1}")} if k < 6 else
             {"1": ("label", f"V{k+1}"), "2": mid, "3": ("GND",)})
    place(f"{PROJECT}:Screw_Terminal_01x03", f"J{k+2}", name, "jlc:CONN-TH_3P-P5.00_KF301-5.0-3P", 495.3, y, sym_t3,
          poles,
          descr=(f"{name} output: +12 V, data and ground, marked on the silk"
                 if data else
                 f"{name} power-injection output: +12 V and ground only; the data pole is intentionally unconnected on the board"),
          lcsc="C474882")

for i in range(4):
    place(f"{PROJECT}:MountingHole", f"H{i+1}", "M3", "MountingHole:MountingHole_3.2mm_M3", 30.48 + 15.24 * i, 330.2,
          sym_hole, {}, descr="Mounting hole M3, 100 x 80 mm pattern")

# ---- sheet notes ------------------------------------------------------------------------------
text("diffrx rev A: RS-422 differential receiver and 12 V / 30 A pixel power distribution board", 20, 18, 3.0)
text("Companion to difftx (FPP Remote pHAT). Cat5 in on J14; four WS2811 data outputs; twelve fused 12 V outputs.", 20, 24, 1.8)
text("RJ45 pairs (same as difftx): port1 = 1(+)/2(-), port2 = 3(+)/6(-), port3 = 4(+)/5(-), port4 = 7(+)/8(-).", 20, 29, 1.8)
text("J1 is the ONLY power inlet: 12 V 30 A straight from the supply. Q1-Q4 block a reversed supply; every", 20, 214, 1.8)
text("output is individually fused by an MF-R400 PPTC (4 A hold / 8 A trip), so no marine fuse panel is needed.", 20, 219, 1.8)
text("Data ports J2 (P1), J5 (P2), J8 (P3), J11 (P4). The eight INJ terminals feed power only:", 400, 20, 1.8)
text("their middle pole is a no-connect so an injection pigtail's data wire never becomes a stub on the string.", 400, 25, 1.8)
text("Terminal poles are silk marked + / D / - and read left to right in that order on both rows of the board.", 400, 30, 1.8)

sch = "\n".join(["(kicad_sch", "\t(version 20260306)", '\t(generator "eeschema")', '\t(generator_version "10.0")',
                 f'\t(uuid "{ROOT_UUID}")', '\t(paper "A2")',
                 '\t(title_block\n\t\t(title "diffrx - RS-422 pixel receiver and 12 V distribution")\n\t\t(date "2026-09-20")\n\t\t(rev "A")\n\t)',
                 LIB_SYMBOLS, *items, '\t(sheet_instances\n\t\t(path "/"\n\t\t\t(page "1")\n\t\t)\n\t)', '\t(embedded_fonts no)', ")"])
open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), "w", encoding="utf-8", newline="\n").write(sch)

lib = '(kicad_symbol_lib\n\t(version 20251024)\n\t(generator "kicad_symbol_editor")\n\t(generator_version "10.0")\n'
for b in [sym_rx, sym_reg, sym_fet, sym_esd, sym_tvs, sym_zen, sym_led, sym_cp, sym_rj,
          sym_fuse, sym_mainfuse, sym_t3, sym_t2, sym_r, sym_c, sym_hole]:
    lib += b.replace(f'(symbol "{PROJECT}:', '(symbol "', 1) + "\n"
open(os.path.join(HERE, f"{PROJECT}.kicad_sym"), "w", encoding="utf-8", newline="\n").write(lib + ")\n")

pro = {"board": {"design_settings": {"defaults": {}, "diff_pair_dimensions": [], "drc_exclusions": [], "rules": {}, "track_widths": [], "via_dimensions": []}},
       "boards": [], "cvpcb": {"equivalence_files": []}, "libraries": {"pinned_footprint_libs": [], "pinned_symbol_libs": []},
       "meta": {"filename": f"{PROJECT}.kicad_pro", "version": 3},
       "net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.8, "via_drill": 0.4, "bus_width": 12, "wire_width": 6,
                                     "line_style": 0, "microvia_diameter": 0.3, "microvia_drill": 0.1, "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25, "diff_pair_width": 0.2,
                                     "pcb_color": "rgba(0, 0, 0, 0.000)", "schematic_color": "rgba(0, 0, 0, 0.000)", "priority": 2147483647}],
                        "meta": {"version": 4}, "net_colors": None, "netclass_assignments": None, "netclass_patterns": []},
       "pcbnew": {"page_layout_descr_file": ""}, "schematic": {"legacy_lib_dir": "", "legacy_lib_list": []}, "sheets": [[ROOT_UUID, "Root"]], "text_variables": {}}
# DRC rules, set here so a regeneration cannot quietly drop back to KiCad's permissive defaults.
# These are JLCPCB's published limits for a 4-layer board in 1 oz outer copper (jlcpcb.com/capabilities):
# 0.15 mm minimum PTH annular ring, 0.127 mm trace/space (0.2 used here for margin), 1.0 mm silk text.
# min_connection matters as much as any of them: at 0.0 KiCad does not check that a track actually
# meets a pad or another track with full width, which is how a 0.1 mm neck passes unnoticed.
pro["board"]["design_settings"]["rules"].update({
    "min_clearance": 0.2,
    "min_connection": 0.2,
    "min_copper_edge_clearance": 0.3,
    "min_hole_clearance": 0.25,
    "min_hole_to_hole": 0.25,
    "min_silk_clearance": 0.15,
    "min_text_height": 1.0,
    "min_text_thickness": 0.15,
    "min_through_hole_diameter": 0.3,
    "min_track_width": 0.2,
    "min_via_annular_width": 0.15,
    "min_via_diameter": 0.6,
})
open(os.path.join(HERE, f"{PROJECT}.kicad_pro"), "w", encoding="utf-8", newline="\n").write(json.dumps(pro, indent=2))
open(os.path.join(HERE, "fp-lib-table"), "w", newline="\n").write("""(fp_lib_table
  (version 7)
  (lib (name "jlc")(type "KiCad")(uri "${KIPRJMOD}/jlc/jlc.pretty")(options "")(descr "JLCPCB footprints via easyeda2kicad"))
  (lib (name "diffrx")(type "KiCad")(uri "${KIPRJMOD}/diffrx.pretty")(options "")(descr "project footprints"))
)
""")
open(os.path.join(HERE, "sym-lib-table"), "w", newline="\n").write(
    f'(sym_lib_table\n\t(version 7)\n\t(lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.kicad_sym")(options "")(descr "Project symbols"))\n)\n')
print("wrote diffrx rev A schematic; root uuid", ROOT_UUID)
