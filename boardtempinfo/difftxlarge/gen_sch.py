"""
Schematic generator for difftxlarge rev A: the 60-output big sibling of difftx (FPP Remote pHAT).

  python gen_sch.py

Outputs: difftxlarge.kicad_sch, difftxlarge.kicad_pro, difftxlarge.kicad_sym, fp-lib-table, sym-lib-table

What the board is (DESIGN.md has the reasoning, RESEARCH.md the sources):
  J16   2x20 shrouded box header, 40-way ribbon to any 40-pin Raspberry Pi. Straight-through numbering.
        GPIO4..GPIO23 = PI_D0..PI_D19 (DPI_D0..D19), GPIO27/26/25 = PI_LE0/1/2 (P1-13/37/22),
        GPIO2/3 = i2c-1, pins 1/17 = the Pi's 3V3. Pins 2/4 (5 V) are NOT connected: a ribbon cannot carry
        a Pi 4/5's supply current, so the Pi is powered from J18 instead.
  RP*   4.7 k pull-downs on all 23 Pi lines (the Pi boots GPIO0-8 with pull-ups; a missing Pi reads LOW)
  U25-27 SN74AHCT541 buffers, 3.3 V in / 5 V out, RS1-23 33 R series into the 23-line bus
  U16-24 SN74AHCT573 latches. Bank b (LE_b) = U(16+3b)..U(18+3b); latch m of a bank takes D(8m..8m+7).
        FPP's latch pulse is 52 ns high with 26 ns data hold (DPIPixels.h); the AHCT573 needs 5 ns / 1.5 ns.
  Output k = 0..59 (FPP port k+1): bank k//20, data bit k%20 -> O(k+1) -> jack J(k//4+1), port k%4+1.
  U1-15 AM26C31 per jack, difftx's channel map: port1 1A->1Y/1Z->RJ45 1/2, port2 3A->3/6, port3 2A->5(+)/4(-) (Falcon: blue pair is 5+/4-),
        port4 4A->7/8. G and ~G tied low (enabled), exactly as difftx rev D.
  D1-60 PSM712 per pair (+12/-7 V stand-off, the receiver's common-mode window, as on diffsmart)
  LED1-60 activity LED per output, off the latch output through RA 2 k.

  Power: J17 12 V -> F1 ATO blade fuse -> Q1 AO4407A high-side P-FET (reverse polarity) -> R_SH 10 mohm
  (INA226 U30) -> +12V. U28 TPS56637 -> 5V_PI -> J18 USB-C power out (Rp 10 k = 3.0 A) -> the Pi's own USB-C.
  U29 TPS56637 -> 5V_DRV (buffers, latches, drivers, LEDs; 60 terminated pairs ~ 1.5 A).
  I2C on the Pi's 3V3: U35 AT24C256 cape EEPROM 0x50, U31 DS3231M RTC 0x68 + BT1 CR2032, U30 INA226 0x40,
  U32/U33 LM75B 0x48/0x49, J22 OLED header (SSD1306 0x3C).
  U34 LM2903 + NTC: fan on ~43 C, OVER TEMP LED ~65 C (diffsmart's circuit), J21 fan output.
  J19 3.5 mm jack (patch cable from the Pi 3/4 audio out, or a USB sound card on a Pi 5) -> J20 line out.
"""
import os, re, uuid, json

HERE = os.path.dirname(os.path.abspath(__file__))
KISYM = r"C:\Program Files\KiCad\10.0\share\kicad\symbols"
PROJECT = "difftxlarge"

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
    """A raw newline inside a quoted string makes KiCad fail the whole file (see diffsmart)."""
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

def flatten_units(block, name, dy):
    """Fold unit 2 of a two-unit EasyEDA symbol into unit 1, shifted down by dy (see diffsmart)."""
    def shift(m):
        return f"({m.group(1)} {m.group(2)} {float(m.group(3)) - dy:g}"
    u2 = block_at(block, block.index(f'(symbol "{name}_2_1"'))
    body = u2[u2.index("\n"):u2.rstrip().rindex(")")]
    body = re.sub(r"\((at|xy|start|end|center|mid) (-?[\d.]+) (-?[\d.]+)", shift, body)
    block = block.replace(u2, "")
    u1 = block_at(block, block.index(f'(symbol "{name}_1_1"'))
    return block.replace(u1, u1[:u1.rstrip().rindex(")")] + body + "\n\t\t)")

SYMS = {}
def S(key, block):
    SYMS[key] = block
    return block

sym_drv   = S("drv",   jlc_symbol("AM26C31IDR", "AM26C31", "AM26C31IDR"))
sym_latch = S("latch", jlc_symbol("SN74AHCT573PWR", "AHCT573", "SN74AHCT573PWR"))
sym_buf   = S("buf",   jlc_symbol("SN74AHCT541PWR", "AHCT541", "SN74AHCT541PWR"))
sym_esd   = S("esd",   jlc_symbol("PSM712-LF-T7", "PSM712", "PSM712"))
sym_rj    = S("rj",    jlc_symbol("R-RJ45R08P-A004", "RJ45_8P8C", "R-RJ45R08P-A004"))
sym_ledg  = S("ledg",  jlc_symbol("0805G", "LED_G", "green"))                  # pin 1 = A, pin 2 = K
sym_ledr  = S("ledr",  jlc_symbol("FC-2012HRK-620D", "LED_R", "red"))          # pin 2 = +, pin 1 = -  (NOT like the green)
sym_buck  = S("buck",  jlc_symbol("TPS56637RPAR", "TPS56637", "TPS56637RPAR"))
sym_ind   = S("ind",   jlc_symbol("PSPMAA0805-3R3M-ANP", "L_3u3", "3.3uH"))
sym_pfet  = S("pfet",  jlc_symbol("AO4407A", "AO4407A", "AO4407A"))
sym_zen   = S("zen",   jlc_symbol("BZT52C15", "BZT52C15", "BZT52C15"))
sym_tvs   = S("tvs",   jlc_symbol("SMDJ15A", "SMDJ15A", "SMDJ15A"))
sym_cp    = S("cp",    jlc_symbol("RST470UF25V032", "CP", "470uF"))
sym_ato   = S("ato",   jlc_symbol("3557-2", "Fuse_ATO", "ATO fuse holder"))
sym_t2    = S("t2",    jlc_symbol("KF301-5.0-2P", "Term2", "terminal 2P 5.0mm"))
sym_t3    = S("t3",    jlc_symbol("KF301-5.0-3P", "Term3", "terminal 3P 5.0mm"))
sym_shunt = S("shunt", jlc_symbol("HOJLR2512-3W-10MR-1%", "Shunt", "10mR"))
sym_ina   = S("ina",   jlc_symbol("INA226AIDGSR", "INA226", "INA226AIDGSR"))
sym_rtc   = S("rtc",   jlc_symbol("DS3231MZ+", "DS3231M", "DS3231MZ+TRL"))
sym_bat   = S("bat",   jlc_symbol("CR2032-BS-6-1", "CR2032", "CR2032 holder"))
sym_idc   = S("idc",   jlc_symbol("HDR-IDC-2.54-2X20P", "PiRibbon", "2x20 box header"))
sym_jack  = S("jack",  jlc_symbol("PJ-3270-4A", "Jack35", "PJ-3270-4A"))
sym_usbc  = S("usbc",  jlc_symbol("TYPE-C6P", "USBC_PWR", "USB-C power out"))
sym_lm75  = S("lm75",  jlc_symbol("LM75BD,118", "LM75B", "LM75BD"))
sym_nfet  = S("nfet",  jlc_symbol("AO3400A", "AO3400A"))
sym_schk  = S("schk",  jlc_symbol("SS34_C8678", "SS34", "SS34"))
sym_hdr4  = S("hdr4",  jlc_symbol("Header-Male-2.54_1x4", "Conn_01x04", "OLED"))
sym_eep   = S("eep",   jlc_symbol("AT24C256C-SSHL-T", "AT24C256", "AT24C256C"))
sym_cmp   = S("cmp",   flatten_units(jlc_symbol("LM2903QDRQ1", "LM2903", "LM2903QDRQ1"), "LM2903", 20.32))
sym_r     = S("r",     rename_symbol(lib_symbol("Device.kicad_sym", "R"), "R", "R"))
sym_c     = S("c",     rename_symbol(lib_symbol("Device.kicad_sym", "C"), "C", "C"))
sym_pptc  = S("pptc",  rename_symbol(lib_symbol("Device.kicad_sym", "Polyfuse"), "Polyfuse", "Polyfuse"))
sym_d     = S("d",     rename_symbol(lib_symbol("Diode.kicad_sym", "1N4148W"), "1N4148W", "1N4148W"))
sym_hole  = S("hole",  rename_symbol(lib_symbol("Mechanical.kicad_sym", "MountingHole"), "MountingHole", "MountingHole"))
sym_tp    = S("tp",    rename_symbol(lib_symbol("Connector.kicad_sym", "TestPoint"), "TestPoint", "TestPoint"))
sym_jp    = S("jp",    rename_symbol(lib_symbol("Jumper.kicad_sym", "SolderJumper_2_Open"), "SolderJumper_2_Open", "SolderJumper_2_Open"))

pw = open(os.path.join(KISYM, "power.kicad_sym"), encoding="utf-8").read()
def power_sym(name):
    return lib_block(pw, name).replace(f'(symbol "{name}"', f'(symbol "power:{name}"', 1)
POWER_SYMS = [power_sym(n) for n in ("GND", "+12V", "PWR_FLAG")]

LIB_SYMBOLS = "\t(lib_symbols\n" + "\n".join(list(SYMS.values()) + POWER_SYMS) + "\n\t)"

# ----------------------------------------------------------------------
# schematic helpers
# ----------------------------------------------------------------------
ROOT_UUID = U()
items = []
pwr_count = [0]

def sym_instance(lib_id, ref, value, footprint, x, y, pins, extra_props=None, datasheet="", descr="", in_bom=True):
    b = "yes" if in_bom else "no"
    lines = [f'\t(symbol\n\t\t(lib_id "{lib_id}")\n\t\t(at {x:g} {y:g} 0)\n\t\t(unit 1)\n\t\t(body_style 1)\n\t\t(exclude_from_sim no)\n\t\t(in_bom {b})\n\t\t(on_board yes)\n\t\t(in_pos_files {b})\n\t\t(dnp no)\n\t\t(uuid "{U()}")']
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
    sym_instance(f"power:{kind}", f"#PWR{pwr_count[0]:03d}", kind, "", x, y, ["1"], descr=f"Power symbol {kind}", in_bom=False)
def pwr_flag(x, y):
    pwr_count[0] += 1
    sym_instance("power:PWR_FLAG", f"#FLG{pwr_count[0]:03d}", "PWR_FLAG", "", x, y, ["1"], descr="Power flag", in_bom=False)

OUT = {0: (-1, 0), 180: (1, 0), 90: (0, 1), 270: (0, -1)}
G = ("GND",)
def L(name):
    return ("label", name)

def place(lib_id, ref, value, footprint, x, y, block, conn, datasheet="", descr="", lcsc="", stub=2.54, in_bom=True):
    x, y = round(round(x / 2.54) * 2.54, 2), round(round(y / 2.54) * 2.54, 2)     # connection grid
    pins = pins_of(block)
    sym_instance(lib_id, ref, value, footprint, x, y, list(pins.keys()), datasheet=datasheet, descr=descr,
                 extra_props={"LCSC": lcsc} if lcsc else None, in_bom=in_bom)
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
        elif c[0] in ("GND", "+12V"):
            power(c[0], ex, ey)
            if len(c) > 1 and c[1] == "flag":
                pwr_flag(ex, ey)
    unused = set(conn) - set(pins)
    assert not unused, (ref, unused)

R0603 = "Resistor_SMD:R_0603_1608Metric"
C0603 = "Capacitor_SMD:C_0603_1608Metric"
C0805 = "Capacitor_SMD:C_0805_2012Metric"
C1206 = "Capacitor_SMD:C_1206_3216Metric"
FP_LEDG = "jlc:LED0805-R-RD"
FP_LEDR = "jlc:LED0805-RD"
P = f"{PROJECT}:"

def res(ref, value, x, y, a, b, descr, lcsc, fp=R0603):
    place(P + "R", ref, value, fp, x, y, sym_r, {"1": a, "2": b}, descr=descr, lcsc=lcsc)
def cap(ref, value, x, y, a, b, descr, lcsc, fp=C0603):
    place(P + "C", ref, value, fp, x, y, sym_c, {"1": a, "2": b}, descr=descr, lcsc=lcsc)
def led_g(ref, x, y, a, k, descr):
    place(P + "LED_G", ref, "green", FP_LEDG, x, y, sym_ledg, {"1": a, "2": k}, descr=descr, lcsc="C2297")
def led_r(ref, x, y, a, k, descr):
    place(P + "LED_R", ref, "red", FP_LEDR, x, y, sym_ledr, {"2": a, "1": k}, descr=descr, lcsc="C84256")

R_ = {"2k": "C22975", "4.7k": "C23162", "10k": "C25804", "33R": "C23140", "100k": "C25803", "75k": "C23242",
      "1k": "C21190", "15k": "C22809", "20k": "C4184", "100R": "C22775"}
C100N, C100N_0805, C100P, C10U, C22U = "C14663", "C49678", "C14858", "C13585", "C12891"

# GPIO -> 40-pin header pin
HDR = {4: 7, 5: 29, 6: 31, 7: 26, 8: 24, 9: 21, 10: 19, 11: 23, 12: 32, 13: 33, 14: 8, 15: 10, 16: 36, 17: 11,
       18: 12, 19: 35, 20: 38, 21: 40, 22: 15, 23: 16, 24: 18, 25: 22, 26: 37, 27: 13}
LE_GPIO = [27, 26, 25]                    # LE0 (bank 0 = J1-J5), LE1, LE2 - the order of "latches" in the EEPROM
LINES = [(f"D{i}", 4 + i) for i in range(20)] + [(f"LE{b}", LE_GPIO[b]) for b in range(3)]

# ======================================================================
# 1. Pi ribbon header, pull-downs, buffers, series resistors
# ======================================================================
conn = {"1": L("P3V3"), "17": L("P3V3"), "3": L("SDA"), "5": L("SCL")}
for g in ("6", "9", "14", "20", "25", "30", "34", "39"):
    conn[g] = ("GND", "flag") if g == "6" else G
for name, gpio in LINES:
    conn[str(HDR[gpio])] = L(f"PI_{name}")
place(P + "PiRibbon", "J16", "2x20 box header, Pi ribbon", "jlc:IDC-TH_40P-P2.54_C9138", 40, 60, sym_idc, conn,
      descr="2x20 2.54 mm shrouded box header for a 40-way IDC ribbon to any 40-pin Raspberry Pi (3B+/4/5). The Pi is "
            "NOT part of the assembly. Straight-through numbering: board pin n = Pi pin n. Pins 2/4 (5 V) are "
            "deliberately unconnected - the Pi is powered from J18 (USB-C) because two ribbon conductors cannot "
            "carry a Pi 4/5's supply. GPIO4-23 = pixel data D0-D19, GPIO27/26/25 = latch enables, GPIO2/3 = i2c-1",
      lcsc="C9138")

for k, (name, gpio) in enumerate(LINES):
    res(f"RP{k+1}", "4.7k", 110, 20 + 17.78 * k, L(f"PI_{name}"), G,
        f"Pull-down on PI_{name} (GPIO{gpio}): holds the line low while the Pi boots (GPIO0-8 come up with ~50 k "
        f"pull-ups: 4.7 k against 50 k is 0.28 V, under the AHCT541's 0.8 V V_IL) and when no Pi is fitted", R_["4.7k"])

# Which buffer channel carries which line is a layout choice: channels are handed out in the order the
# lines' pins sit along the header (left to right on the board, where pin 1 is at the right end), so the
# 23 header-to-buffer tracks run side by side instead of crossing. gen_pcb.py computes the same SLOT.
def hdr_col(gpio):
    return -((HDR[gpio] - 1) // 2)
ORDER_BY_X = sorted(range(len(LINES)), key=lambda k: (hdr_col(LINES[k][1]), HDR[LINES[k][1]] % 2))
SLOT = {LINES[k][0]: s for s, k in enumerate(ORDER_BY_X)}          # slot 0 = leftmost channel on the board
for u in range(3):
    conn = {"1": G, "19": G, "10": G, "20": L("5V_DRV")}
    for t in range(8):                                  # t = position left to right = A(8-t), pin 9-t
        a_pin, y_pin = str(9 - t), str(11 + t)
        names = [n for n, s_ in SLOT.items() if s_ == 8 * u + t]
        if names:
            conn[a_pin] = L(f"PI_{names[0]}"); conn[y_pin] = L(f"B_{names[0]}")
        else:
            conn[a_pin] = G                              # unused input tied low, output left open
    place(P + "AHCT541", f"U{25+u}", "SN74AHCT541PWR", "jlc:TSSOP-20_L6.5-W4.4-P0.65-LS6.4-BL", 160, 40 + 130 * u,
          sym_buf, conn, datasheet="https://www.ti.com/lit/ds/symlink/sn74ahct541.pdf",
          descr="Octal buffer, 5 V supply, TTL inputs: turns the Pi's 3.3 V GPIO into a 5 V bus able to drive 3 latches "
                "across the board, and keeps the long bus off the Pi's pins", lcsc="C50989")
    cap(f"C{1+u}", "100nF", 190, 25 + 130 * u, L("5V_DRV"), G, f"U{25+u} decoupling", C100N)

for k, (name, gpio) in enumerate(LINES):
    res(f"RS{k+1}", "33R", 230, 20 + 17.78 * k, L(f"B_{name}"), L(name),
        f"Series source termination for bus line {name}: damps ringing on a ~300 mm unterminated trace that carries "
        f"26 ns latch pulses", R_["33R"])

# ======================================================================
# 2. latches
# ======================================================================
for b in range(3):
    for m in range(3):
        ref = f"U{16 + 3*b + m}"
        conn = {"1": G, "10": G, "20": L("5V_DRV"), "11": L(f"LE{b}")}
        # Bits run 8D -> 1D (bit 0 on 8D, pin 9 / 8Q, pin 12). Rotated 180 degrees on the board, that puts the
        # inputs left to right in bit order facing the bus, and the outputs left to right in jack order facing
        # the drivers, so neither side needs a crossing. A latch does not care which of its eight bits is which.
        # Each jack's LEDs and pairs run port 4, 3, 2, 1 from left to right, so each group of four latch bits
        # is laid in reverse too: output position q (pin 12+q, left to right) carries bit 4*(q//4) + 3 - q%4.
        for q in range(8):
            d = 8 * m + 4 * (q // 4) + 3 - q % 4
            if d < 20:
                conn[str(9 - q)] = L(f"D{d}")
                conn[str(12 + q)] = L(f"O{20*b + d + 1}")
            else:
                conn[str(9 - q)] = G
        x, y = 290 + 60 * m, 40 + 90 * b
        place(P + "AHCT573", ref, "SN74AHCT573PWR", "jlc:TSSOP-20_L6.5-W4.4-P0.65-LS6.4-BL", x, y, sym_latch, conn,
              datasheet="https://www.ti.com/lit/ds/symlink/sn74ahct573.pdf",
              descr=f"Octal transparent latch, bank {b+1} (LE{b}), data D{8*m}-D{min(8*m+7, 19)} -> outputs "
                    f"{20*b + 8*m + 1}-{20*b + min(8*m+8, 20)}. tW(LE) 5 ns / th 1.5 ns min against FPP's 52 ns pulse and 26 ns hold", lcsc="C141311")
        cap(f"C{4 + 3*b + m}", "100nF", x + 25, y - 15, L("5V_DRV"), G, f"{ref} decoupling", C100N)

# ======================================================================
# 3. per-jack: driver, ESD, RJ45, activity LEDs
# ======================================================================
# port p (0..3) -> (driver input pin, Y pin, Z pin, RJ45 + pin, RJ45 - pin). Which of the four driver channels
# serves which pair is a layout choice (difftx used another): with the driver turned 180 degrees over the jack,
# channel 4 (pins 13/14) sits right above pair 1/2 and channel 3 (10/11) above pair 7/8, so those two run
# straight down, and the top-row channels 1 and 2 take the two middle pairs.
CH = [("15", "14", "13", "1", "2"), ("1", "2", "3", "3", "6"), ("7", "6", "5", "5", "4"), ("9", "10", "11", "7", "8")]
for j in range(15):
    n = j + 1
    x0, y0 = 30 + (j % 5) * 130, 470 + (j // 5) * 115
    dconn = {"4": G, "12": G, "8": G, "16": L("5V_DRV")}
    jconn = {}
    for p, (a, yp, zp, rp, rn) in enumerate(CH):
        o = 4 * j + p + 1
        pos, neg = f"J{n}_{p+1}P", f"J{n}_{p+1}N"
        dconn[a] = L(f"O{o}"); dconn[yp] = L(pos); dconn[zp] = L(neg)
        jconn[rp] = L(pos); jconn[rn] = L(neg)
        place(P + "PSM712", f"D{o}", "PSM712", "jlc:SOT-23-3_L3.0-W1.7-P0.95-LS2.9-BR", x0 + 68, y0 + 22 * p, sym_esd,
              {"1": L(pos), "2": L(neg), "3": G}, datasheet="https://www.lcsc.com/datasheet/C32677.pdf",
              descr=f"Asymmetric TVS pair (+12 V / -7 V stand-off) on J{n} port {p+1}, protecting the driver from surges on the outdoor cable",
              lcsc="C32677")
        led_g(f"LED{o}", x0 + 88, y0 + 22 * p, L(f"O{o}"), L(f"LA{o}"), f"Output {o} (J{n}-{p+1}) activity LED, lit while data is clocking out")
        res(f"RA{o}", "2k", x0 + 110, y0 + 22 * p + 5, L(f"LA{o}"), G, f"Output {o} activity LED resistor (about 1 mA)", R_["2k"])
    place(P + "AM26C31", f"U{n}", "AM26C31IDR", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", x0 + 10, y0 + 20, sym_drv, dconn,
          datasheet="https://www.ti.com/lit/ds/symlink/am26c31.pdf",
          descr=f"Quad RS-422 driver for J{n}. Y = +, Z = -. G and ~G tied low = enabled (as difftx rev D). Port 1 = ch4, 2 = ch1, 3 = ch2, 4 = ch3", lcsc="C34923")
    cap(f"C{13 + j}", "100nF", x0 + 10, y0 - 10, L("5V_DRV"), G, f"U{n} decoupling", C100N)
    place(P + "RJ45_8P8C", f"J{n}", "R-RJ45R08P-A004", "jlc:RJ45-TH_R-RJ45R08P-A004", x0 + 45, y0 + 25, sym_rj, jconn,
          datasheet="https://www.lcsc.com/datasheet/C385834.pdf",
          descr=f"J{n}: outputs {4*j+1}-{4*j+4} (FPP J{n}-1..4). Falcon differential pinout: port1 = 1(+)/2(-), port2 = 3(+)/6(-), port3 = 5(+)/4(-), port4 = 7(+)/8(-)",
          lcsc="C385834")

# bulk on 5V_DRV: two, not four. With U29's own 3 x 22 uF and 27 x 100 nF of decoupling, four put the rail at
# about 100 uF effective - the top of TI's range for the TPS56637's loop (independent review, 2026-09-25).
for k in (0, 3):
    cap(f"C{28 + k}", "22uF", 600 + 15 * k, 40, L("5V_DRV"), G, "5V_DRV bulk (one at bank 2, one at the buffers)", C22U, fp=C1206)

# ======================================================================
# 4. 12 V input, fuse, reverse polarity, current sense, TVS, bulk
# ======================================================================
X, Y = 700, 30
place(P + "Term2", "J17", "12V IN", "jlc:CONN-TH_P5.00_KF301-5.0-2P", X, Y, sym_t2, {"1": L("12VIN"), "2": G},
      descr="12 V DC input, 5.0 mm screw terminal. Pin 1 = +12 V, pin 2 = GND (silk marked)", lcsc="C474881")
place(P + "Fuse_ATO", "F1", "ATO 5A", "jlc:FUSE-TH_4P-L19.8-W6.7_3557-2", X + 30, Y, sym_ato,
      {"1": L("12VF"), "2": L("12VF"), "3": L("12VIN"), "4": L("12VIN")},
      datasheet="https://www.lcsc.com/datasheet/C352820.pdf",
      descr="Keystone 3557-2 ATO blade fuse holder. Fit a 5 A ATO fuse: the board draws ~3.5 A at 12 V with a Pi 5 at full load",
      lcsc="C352820")
place(P + "AO4407A", "Q1", "AO4407A", "jlc:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", X + 60, Y, sym_pfet,
      {"1": L("12VP"), "2": L("12VP"), "3": L("12VP"), "4": L("QG"), "5": L("12VF"), "6": L("12VF"), "7": L("12VF"), "8": L("12VF")},
      datasheet="https://www.lcsc.com/datasheet/C16072.pdf",
      descr="Reverse-polarity P-FET, high side: drain to the supply, source to the board. The body diode conducts first, "
            "then the gate (pulled to ground) turns it on at -12 V Vgs. Reversed, Vgs is positive and it stays off. "
            "13 mohm, 0.3 W at 5 A", lcsc="C16072")
res("R1", "10k", X + 60, Y + 25, L("QG"), G, "Q1 gate pull-down", R_["10k"])
place(P + "BZT52C15", "D61", "BZT52C15", "jlc:SOD-123_L2.7-W1.6-LS3.7-RD", X + 80, Y + 25, sym_zen,
      {"1": L("12VP"), "2": L("QG")}, datasheet="https://www.lcsc.com/datasheet/C173427.pdf",
      descr="15 V zener gate-source clamp for Q1 (pin 1 = cathode on the source)", lcsc="C173427")
place(P + "SMDJ15A", "D62", "SMDJ15A", "jlc:SMC_L6.9-W5.9-LS7.9-RD", X + 100, Y, sym_tvs,
      {"1": L("12VP"), "2": G}, datasheet="https://www.lcsc.com/datasheet/C152077.pdf",
      descr="3 kW 15 V TVS behind the reverse-polarity FET (ahead of it, it would short a reversed supply)", lcsc="C152077")
place(P + "Shunt", "R2", "10mR", "jlc:RES-SMD_L6.4-W3.2-R2512", X + 125, Y, sym_shunt,
      {"1": L("12VP"), "2": ("+12V", "flag")}, datasheet="https://www.lcsc.com/datasheet/C2903468.pdf",
      descr="INA226 current shunt, 10 mohm 1% metal alloy 2512 - the value Linux's ina2xx driver assumes. 0.25 W at 5 A",
      lcsc="C2903468")
place(P + "CP", "C32", "470uF 25V", "jlc:CAP-SMD_BD8.0-L8.3-W8.3-FD", X + 150, Y, sym_cp, {"1": ("+12V",), "2": G},
      datasheet="https://www.lcsc.com/datasheet/C4747956.pdf", descr="Bulk capacitor on the 12 V bus (pin 1 = +)", lcsc="C4747956")
res("R3", "10k", X + 170, Y, ("+12V",), G, "Bleeder so C32 does not sit charged after power-down", R_["10k"])
# reversed-supply indicator: across the input terminal, blocked by D63 in the normal direction
led_r("LED61", X, Y + 40, G, L("RPA"), "REVERSED: lit when the 12 V supply is connected backwards")
place(P + "1N4148W", "D63", "1N4148W", "Diode_SMD:D_SOD-123", X + 20, Y + 40, sym_d, {"2": L("RPA"), "1": L("RPK")},
      descr="Blocks the 12 V that would otherwise reverse-bias LED61 in normal operation (LEDs are rated 5 V reverse)",
      lcsc="C81598")
res("R4", "4.7k", X + 40, Y + 40, L("RPK"), L("12VIN"), "REVERSED LED series resistor, ~2 mA", R_["4.7k"])
led_g("LED62", X + 60, Y + 40, ("+12V",), L("LK12"), "12V: board supply present (after fuse and reverse protection)")
res("R5", "4.7k", X + 80, Y + 40, L("LK12"), G, "12V LED series resistor", R_["4.7k"])

# ======================================================================
# 5. two TPS56637 bucks: 5V_PI and 5V_DRV
# ======================================================================
def buck(u, rail, x, y, cbase, rbase, lref):
    """TI TPS56637 5.1 V / 6 A, values from the datasheet's 5 V design (Table 4, Figure 17):
    L 3.3 uH, Cout >= 20 uF effective (3 x 22 uF 1206 X5R ~ 30 uF at 5 V), feed-forward 20 k + 100 pF across the
    top divider resistor, BOOT 100 nF, VFB 0.600 V: 75 k / 10 k -> 5.100 V. EN divider 100 k / 15 k -> starts
    ~8.9 V, stops ~8.1 V (EN abs max 6 V; 12 V puts it at 1.6 V). MODE open = forced CCM. PG unused."""
    sw, fb, en, bt, ff = f"SW_{rail}", f"FB_{rail}", f"EN_{rail}", f"BT_{rail}", f"FF_{rail}"
    place(P + "TPS56637", u, "TPS56637RPAR", "jlc:VQFN-HR-10_L3.0-W3.0_RPA", x, y, sym_buck,
          {"1": L(en), "2": L(fb), "3": G, "4": ("NC",), "5": ("NC",), "6": L(sw), "7": L(bt), "8": ("+12V",), "9": G, "10": ("NC",)},
          datasheet="https://www.ti.com/lit/ds/symlink/tps56637.pdf",
          descr=f"4.5-28 V in, 6 A synchronous buck -> {rail} 5.1 V", lcsc="C841386")
    place(P + "L_3u3", lref, "3.3uH", "jlc:IND-SMD_L8.5-W8.0_PSPMAA0803H-330M-ANP", x + 40, y, sym_ind,
          {"1": L(sw), "2": L(rail)}, datasheet="https://www.lcsc.com/datasheet/C2962881.pdf",
          descr="3.3 uH, 11 mohm, Isat 17 A (valley limit 8.6 A max + ripple)", lcsc="C2962881")
    cap(f"C{cbase}", "100nF", x + 20, y - 20, L(bt), L(sw), f"{u} bootstrap capacitor", C100N)
    for i in range(3):
        cap(f"C{cbase+1+i}", "10uF", x - 40 + 10 * i, y + 25, ("+12V",), G, f"{u} input capacitor, 10 uF 25 V X5R 1206", C10U, fp=C1206)
    cap(f"C{cbase+4}", "100nF", x - 10, y + 25, ("+12V",), G, f"{u} input HF bypass, at VIN/PGND", C100N)
    for i in range(3):
        cap(f"C{cbase+5+i}", "22uF", x + 60 + 10 * i, y + 25, L(rail), G, f"{u} output capacitor, 22 uF 25 V X5R 1206", C22U, fp=C1206)
    cap(f"C{cbase+8}", "100pF", x + 20, y + 45, L(rail), L(ff), f"{u} feed-forward capacitor (with R{rbase+2})", C100P)
    res(f"R{rbase}", "75k", x + 40, y + 45, L(rail), L(fb), f"{u} feedback, top: 0.6 V x (1 + 75/10) = 5.10 V", R_["75k"])
    res(f"R{rbase+1}", "10k", x + 55, y + 45, L(fb), G, f"{u} feedback, bottom", R_["10k"])
    res(f"R{rbase+2}", "20k", x + 70, y + 45, L(ff), L(fb), f"{u} feed-forward resistor (datasheet R8)", R_["20k"])
    res(f"R{rbase+3}", "100k", x + 85, y + 45, ("+12V",), L(en), f"{u} EN divider, top (UVLO ~8.9 V on / 8.1 V off)", R_["100k"])
    res(f"R{rbase+4}", "15k", x + 100, y + 45, L(en), G, f"{u} EN divider, bottom", R_["15k"])

buck("U28", "5V_PI", 740, 130, 40, 10, "L1")
buck("U29", "5V_DRV", 740, 230, 50, 20, "L2")

# USB-C power out to the Pi
place(P + "USBC_PWR", "J18", "USB-C to Pi", "jlc:TYPE-C-SMD_TYPE-C-6P-073", 900, 130, sym_usbc,
      {"A9": L("5V_PI"), "B9": L("5V_PI"), "A12": G, "B12": G, "7": G, "A5": L("CC1"), "B5": L("CC2")},
      datasheet="https://www.lcsc.com/datasheet/C668623.pdf",
      descr="USB-C receptacle, POWER OUT: a short C-to-C cable from here to the Pi's own USB-C input powers the Pi "
            "(5.1 V, 3 A advertised). Pi 5: add usb_max_current_enable=1 to config.txt for full USB current",
      lcsc="C668623")
res("R30", "10k", 930, 120, L("5V_PI"), L("CC1"), "Rp on CC1: 10 k to 5 V advertises 3.0 A (USB Type-C source)", R_["10k"])
res("R31", "10k", 930, 135, L("5V_PI"), L("CC2"), "Rp on CC2: 10 k to 5 V advertises 3.0 A (USB Type-C source)", R_["10k"])
led_g("LED63", 900, 160, L("5V_PI"), L("LKPI"), "5V PI: the Pi supply rail is up")
res("R32", "1k", 920, 160, L("LKPI"), G, "5V PI LED series resistor", R_["1k"])
led_g("LED64", 900, 250, L("5V_DRV"), L("LKDRV"), "5V DRV: the driver supply rail is up")
res("R33", "1k", 920, 250, L("LKDRV"), G, "5V DRV LED series resistor", R_["1k"])

# ======================================================================
# 6. I2C: EEPROM, RTC, INA226, LM75s, OLED header
# ======================================================================
X, Y = 1000, 30
res("R34", "4.7k", X, Y, L("P3V3"), L("SDA"), "I2C pull-up (parallels the Pi's own 1.8 k)", R_["4.7k"])
res("R35", "4.7k", X + 15, Y, L("P3V3"), L("SCL"), "I2C pull-up (parallels the Pi's own 1.8 k)", R_["4.7k"])
place(P + "AT24C256", "U35", "AT24C256C", "jlc:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", X, Y + 30, sym_eep,
      {"1": G, "2": G, "3": G, "4": G, "5": L("SDA"), "6": L("SCL"), "7": L("WP"), "8": L("P3V3")},
      datasheet="https://ww1.microchip.com/downloads/en/devicedoc/atmel-8568-seeprom-at24c256c-datasheet.pdf",
      descr="FPP cape EEPROM, 0x50 on i2c-1 (CapeUtils.cpp: PLATFORM_PI I2C_DEV 1). Image from make_eeprom.py", lcsc="C6482")
cap("C60", "100nF", X + 25, Y + 20, L("P3V3"), G, "U35 decoupling", C100N)
res("R36", "10k", X + 40, Y + 40, L("WP"), G, "EEPROM WP pull-down: writable, so FPP can program and sign it in place", R_["10k"])
place(P + "SolderJumper_2_Open", "JP1", "WP", "Jumper:SolderJumper-2_P1.3mm_Open_Pad1.0x1.5mm", X + 40, Y + 25, sym_jp,
      {"1": L("WP"), "2": L("P3V3")}, descr="Close after signing to write-protect the EEPROM (WP high)")

place(P + "DS3231M", "U31", "DS3231MZ+TRL", "jlc:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", X, Y + 70, sym_rtc,
      {"1": ("NC",), "2": L("P3V3"), "3": ("NC",), "4": ("NC",), "5": G, "6": L("VBAT"), "7": L("SDA"), "8": L("SCL")},
      datasheet="https://www.analog.com/media/en/technical-documentation/data-sheets/DS3231M.pdf",
      descr="MEMS RTC +/-5 ppm, 0x68. FPP setting piRTC = 2 (DS1307/DS3231); the cape registers it as ds3231", lcsc="C107410")
cap("C61", "100nF", X + 25, Y + 60, L("P3V3"), G, "U31 decoupling", C100N)
place(P + "CR2032", "BT1", "CR2032 holder", "jlc:BAT-TH_CR2032-BS-6-1", X + 40, Y + 70, sym_bat, {"1": L("VBAT"), "2": G},
      datasheet="https://www.lcsc.com/datasheet/C70377.pdf",
      descr="CR2032 holder, SMT. Pad 1 = + (the holder's drawing and silk). Keeps the RTC running without the Pi", lcsc="C70377")

place(P + "INA226", "U30", "INA226AIDGSR", "jlc:MSOP-10_L3.0-W3.0-P0.50-LS5.0-BL", X, Y + 110, sym_ina,
      {"1": G, "2": G, "3": ("NC",), "4": L("SDA"), "5": L("SCL"), "6": L("P3V3"), "7": G, "8": ("+12V",),
       "9": ("+12V",), "10": L("12VP")},
      datasheet="https://www.ti.com/lit/ds/symlink/ina226.pdf",
      descr="12 V input voltage/current monitor, 0x40 (A0 = A1 = GND). IN+ = supply side of R2, IN- and VBUS = +12V. "
            "Full scale 81.92 mV / 10 mohm = 8.2 A", lcsc="C49851")
cap("C62", "100nF", X + 25, Y + 100, L("P3V3"), G, "U30 decoupling", C100N)

place(P + "LM75B", "U32", "LM75BD", "jlc:SOIC-8_L5.0-W4.0-P1.27-LS6.0-BL", X, Y + 150, sym_lm75,
      {"1": L("SDA"), "2": L("SCL"), "3": ("NC",), "4": G, "5": G, "6": G, "7": G, "8": L("P3V3")},
      datasheet="https://www.lcsc.com/datasheet/C34565.pdf", descr="Temperature sensor at the driver row, 0x48", lcsc="C34565")
cap("C63", "100nF", X + 25, Y + 140, L("P3V3"), G, "U32 decoupling", C100N)
place(P + "LM75B", "U33", "LM75BD", "jlc:SOIC-8_L5.0-W4.0-P1.27-LS6.0-BL", X, Y + 190, sym_lm75,
      {"1": L("SDA"), "2": L("SCL"), "3": ("NC",), "4": G, "5": G, "6": G, "7": L("P3V3"), "8": L("P3V3")},
      datasheet="https://www.lcsc.com/datasheet/C34565.pdf", descr="Temperature sensor at the power section, 0x49 (A0 high)", lcsc="C34565")
cap("C64", "100nF", X + 25, Y + 180, L("P3V3"), G, "U33 decoupling", C100N)

place(P + "Conn_01x04", "J22", "OLED socket", "jlc:HDR-TH_4P-P2.54-V-F", X, Y + 230, sym_hdr4,
      {"1": G, "2": L("P3V3"), "3": L("SCL"), "4": L("SDA")},
      descr="1x4 2.54 mm FEMALE socket for a 0.96 inch SSD1306 I2C OLED module, in the common GND / VCC / SCL / SDA "
            "order (3.3 V). The module's pins plug in and it lies face up over the outlined area on the board. FPP: "
            "LEDDisplayType 1 = 128x64 SSD1306 at 0x3C. Status display only - no GPIO is left for buttons",
      lcsc="C2718488")

# ======================================================================
# 7. fan thermostat (diffsmart's circuit) and fan output
# ======================================================================
X, Y = 1150, 30
place(P + "LM2903", "U34", "LM2903QDRQ1", "jlc:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", X, Y + 20, sym_cmp,
      {"1": L("FANDRV"), "2": L("NTC"), "3": L("VREFAN"), "4": G, "5": L("NTC"), "6": L("VREFOT"), "7": L("OT"), "8": ("+12V",)},
      datasheet="https://www.lcsc.com/datasheet/C475499.pdf",
      descr="Dual comparator, AEC-Q100: half A switches the fan at ~43 C, half B lights OVER TEMP at ~65 C", lcsc="C475499")
cap("C65", "100nF", X + 30, Y, ("+12V",), G, "LM2903 decoupling", C100N)
res("R37", "10k", X, Y + 60, L("5V_DRV"), L("NTC"), "Top of the NTC divider", R_["10k"])
place(P + "R", "RT1", "10k NTC", "Resistor_SMD:R_0805_2012Metric", X + 15, Y + 60, sym_r, {"1": L("NTC"), "2": G},
      datasheet="https://www.lcsc.com/datasheet/C3195213.pdf",
      descr="Thermostat NTC: Vishay NTCS0805E3103FHT, 10 k at 25 C, B25/85 = 3940 K. 43 C -> 1.60 V, 65 C -> 0.88 V", lcsc="C3195213")
res("R38", "10k", X + 30, Y + 60, L("5V_DRV"), L("VREFAN"), "Fan threshold reference, top", R_["10k"])
res("R39", "4.7k", X + 45, Y + 60, L("VREFAN"), G, "Fan threshold reference, bottom (1.60 V ~ 43 C)", R_["4.7k"])
res("R40", "100k", X + 60, Y + 60, L("FANDRV"), L("VREFAN"), "Fan thermostat hysteresis, about 2.4 C", R_["100k"])
res("R41", "4.7k", X + 75, Y + 60, L("5V_DRV"), L("FANDRV"), "Pull-up for the LM2903's open-drain output", R_["4.7k"])
res("R42", "4.7k", X + 90, Y + 60, L("5V_DRV"), L("VREFOT"), "Over-temp threshold reference, top", R_["4.7k"])
res("R43", "1k", X + 105, Y + 60, L("VREFOT"), G, "Over-temp threshold reference, bottom (0.88 V ~ 65 C)", R_["1k"])
res("R44", "1k", X, Y + 90, L("5V_DRV"), L("OTA"), "OVER TEMP LED series resistor", R_["1k"])
led_r("LED65", X + 20, Y + 90, L("OTA"), L("OT"), "OVER TEMP: lit above about 65 C on the board")
res("R45", "100k", X + 40, Y + 90, L("FANDRV"), G, "Fan FET gate pull-down", R_["100k"])
res("R48", "330k", X + 60, Y + 90, L("OT"), L("NTC"),
    "OVER TEMP hysteresis (positive feedback to IN+B): alarm on at ~65.4 C, off at ~64.4 C. 330 k, not 100 k: "
    "while OT is high this also lifts the NTC node the fan comparator shares, and 330 k keeps that to ~0.7 C", "C23137")
res("R49", "10k", X + 80, Y + 90, L("5V_DRV"), L("OT"),
    "Pull-up on the LM2903's open-collector OT output. Supplies R48's current when the alarm is off, so none of it "
    "flows through LED65 (which would otherwise glow faintly at ~10 uA)", R_["10k"])
place(P + "AO3400A", "Q2", "AO3400A", "jlc:SOT-23-3_L2.9-W1.3-P1.90-LS2.4-BR", X, Y + 120, sym_nfet,
      {"1": L("FANDRV"), "2": G, "3": L("FANM")}, datasheet="https://www.lcsc.com/datasheet/C20917.pdf",
      descr="Fan low-side switch, 30 V 5.7 A", lcsc="C20917")
place(P + "Polyfuse", "F2", "0.35A/30V", "Fuse:Fuse_1206_3216Metric", X + 30, Y + 120, sym_pptc,
      {"1": ("+12V",), "2": L("FAN12")}, datasheet="https://www.lcsc.com/datasheet/C2876002.pdf",
      descr="Fan branch fuse, 0.35 A hold. Keep the fan under 200 mA", lcsc="C2876002")
place(P + "SS34", "D64", "SS34", "jlc:SMA_L4.3-W2.6-LS5.2-RD", X + 60, Y + 120, sym_schk,
      {"1": L("FAN12"), "2": L("FANM")}, datasheet="https://www.lcsc.com/datasheet/C8678.pdf",
      descr="Fan flyback (pin 1 = cathode on the supply)", lcsc="C8678")
place(P + "Term3", "J21", "FAN 12V", "jlc:CONN-TH_3P-P5.00_KF301-5.0-3P", X + 90, Y + 120, sym_t3,
      {"1": L("FAN12"), "2": L("FANM"), "3": G}, descr="Fan output: fused +12 V, switched return, spare ground. Silk + - G",
      lcsc="C474882")

# ======================================================================
# 8. audio: 3.5 mm in from the Pi -> line out terminal
# ======================================================================
X, Y = 1150, 200
# PJ-3270-4A as imported: footprint pad 1 = sleeve, 2 = tip, 3 = ring, 4 = tip normally-closed switch
place(P + "Jack35", "J19", "AUDIO IN", "jlc:AUDIO-TH_PJ-3270-4A", X, Y, sym_jack,
      {"1": G, "2": L("AUD_L_IN"), "3": L("AUD_R_IN"), "4": ("NC",)},
      datasheet="https://www.lcsc.com/datasheet/C961757.pdf",
      descr="3.5 mm stereo jack: patch cable from the Pi 3/4 headphone jack (or a USB sound card on a Pi 5). "
            "Footprint pads: 1 sleeve, 2 tip (L), 3 ring (R), 4 tip switch", lcsc="C961757")
res("R46", "100R", X + 25, Y - 12, L("AUD_L_IN"), L("AUD_L"), "Line out L series resistor: protects the source against a shorted terminal", R_["100R"])
res("R47", "100R", X + 25, Y + 12, L("AUD_R_IN"), L("AUD_R"), "Line out R series resistor", R_["100R"])
place(P + "Term3", "J20", "LINE OUT", "jlc:CONN-TH_3P-P5.00_KF301-5.0-3P", X + 50, Y, sym_t3,
      {"1": L("AUD_L"), "2": L("AUD_R"), "3": G}, descr="Line-level audio out to an external amplifier: L / R / GND (silk marked)",
      lcsc="C474882")

# ======================================================================
# 9. test points, mounting holes, zip-tie slots
# ======================================================================
TPS = [("+12V",), L("5V_PI"), L("5V_DRV"), L("P3V3"), G, G, L("LE0"), L("LE1"), L("LE2"), L("D0"), L("SDA"), L("SCL")]
for i, net in enumerate(TPS):
    place(P + "TestPoint", f"TP{i+1}", "TP", "TestPoint:TestPoint_Pad_D1.5mm", 1300 + 15 * (i % 6), 30 + 20 * (i // 6),
          sym_tp, {"1": net}, descr=f"Test point on {net[-1]}", in_bom=False)
for i in range(6):
    place(P + "MountingHole", f"H{i+1}", "M3", "MountingHole:MountingHole_3.2mm_M3", 1300 + 15 * i, 90, sym_hole, {},
          descr="Enclosure mounting hole, M3", in_bom=False)
for i in range(2):
    place(P + "MountingHole", f"H{i+11}", "M3", "MountingHole:MountingHole_3.2mm_M3", 1300 + 15 * (i + 6), 90, sym_hole, {},
          descr="Enclosure mounting hole, M3, on the top edge between the terminals (they take screwdriver force)", in_bom=False)
for i in range(4):
    place(P + "MountingHole", f"H{i+7}", "M2.5", "MountingHole:MountingHole_2.7mm_M2.5", 1300 + 15 * i, 110, sym_hole, {},
          descr="Raspberry Pi mounting hole, M2.5 (the Pi's own 58 x 49 mm pattern), for standoffs", in_bom=False)
for j in range(15):
    for s in range(2):
        place(P + "MountingHole", f"Z{2*j+s+1}", "tie", f"{PROJECT}:ZipTie_Slot_2x4.5", 1300 + 12 * (j % 8) + 6 * s,
              140 + 15 * (j // 8), sym_hole, {}, descr=f"J{j+1} cable-tie slot, 2.0 x 4.5 mm NPTH", in_bom=False)

# ---- sheet notes ----------------------------------------------------------------------------------
text("difftxlarge rev A - 60-output FPP DPIPixels cape: 15 x RJ45 (Falcon differential), 3 latch banks", 20, 12, 3.0)
text("Output k (FPP port k+1, 0-based k): bank k//20, data line D(k%20) = GPIO(4 + k%20), jack J(k//4+1), port k%4+1.", 20, 18, 1.8)
text("Latch enables: LE0 = GPIO27 (P1-13) -> J1-J5, LE1 = GPIO26 (P1-37) -> J6-J10, LE2 = GPIO25 (P1-22) -> J11-J15.", 20, 22, 1.8)
text("RJ45: port1 = 1(+)/2(-), port2 = 3(+)/6(-), port3 = 5(+)/4(-), port4 = 7(+)/8(-). Falcon standard (difftx rev D had port 3 reversed).", 20, 26, 1.8)
text("The Pi is powered from J18 (USB-C out, 5.1 V) through its own USB-C input, never through the ribbon.", 700, 12, 1.8)

sch = "\n".join(["(kicad_sch", "\t(version 20260306)", '\t(generator "eeschema")', '\t(generator_version "10.0")',
                 f'\t(uuid "{ROOT_UUID}")', '\t(paper "A0")',
                 '\t(title_block\n\t\t(title "difftxlarge - 60-output RS-422 pixel cape")\n\t\t(date "2026-09-25")\n\t\t(rev "A")\n\t)',
                 LIB_SYMBOLS, *items, '\t(sheet_instances\n\t\t(path "/"\n\t\t\t(page "1")\n\t\t)\n\t)', '\t(embedded_fonts no)', ")"])
open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), "w", encoding="utf-8", newline="\n").write(sch)

lib = '(kicad_symbol_lib\n\t(version 20251024)\n\t(generator "kicad_symbol_editor")\n\t(generator_version "10.0")\n'
for b in SYMS.values():
    lib += b.replace(f'(symbol "{PROJECT}:', '(symbol "', 1) + "\n"
open(os.path.join(HERE, f"{PROJECT}.kicad_sym"), "w", encoding="utf-8", newline="\n").write(lib + ")\n")

pro = {"board": {"design_settings": {"defaults": {}, "diff_pair_dimensions": [], "drc_exclusions": [], "rules": {}, "track_widths": [], "via_dimensions": []}},
       "boards": [], "cvpcb": {"equivalence_files": []}, "libraries": {"pinned_footprint_libs": [], "pinned_symbol_libs": []},
       "meta": {"filename": f"{PROJECT}.kicad_pro", "version": 3},
       "net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.7, "via_drill": 0.3, "bus_width": 12, "wire_width": 6,
                                     "line_style": 0, "microvia_diameter": 0.3, "microvia_drill": 0.1, "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25, "diff_pair_width": 0.2,
                                     "pcb_color": "rgba(0, 0, 0, 0.000)", "schematic_color": "rgba(0, 0, 0, 0.000)", "priority": 2147483647}],
                        "meta": {"version": 4}, "net_colors": None, "netclass_assignments": None, "netclass_patterns": []},
       "pcbnew": {"page_layout_descr_file": ""}, "schematic": {"legacy_lib_dir": "", "legacy_lib_list": []}, "sheets": [[ROOT_UUID, "Root"]], "text_variables": {}}
pro["board"]["design_settings"]["rules"].update({
    "min_clearance": 0.2, "min_connection": 0.2, "min_copper_edge_clearance": 0.3, "min_hole_clearance": 0.3,
    "min_hole_to_hole": 0.25, "min_silk_clearance": 0.15, "min_text_height": 1.0, "min_text_thickness": 0.15,
    "min_through_hole_diameter": 0.3, "min_track_width": 0.2, "min_via_annular_width": 0.2, "min_via_diameter": 0.7,
    "min_resolved_spokes": 1,
})
pro["board"]["design_settings"].setdefault("rule_severities", {})["lib_footprint_mismatch"] = "ignore"
open(os.path.join(HERE, f"{PROJECT}.kicad_pro"), "w", encoding="utf-8", newline="\n").write(json.dumps(pro, indent=2))
open(os.path.join(HERE, "fp-lib-table"), "w", newline="\n").write(f"""(fp_lib_table
  (version 7)
  (lib (name "jlc")(type "KiCad")(uri "${{KIPRJMOD}}/jlc/jlc.pretty")(options "")(descr "JLCPCB footprints via easyeda2kicad"))
  (lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.pretty")(options "")(descr "project footprints"))
)
""")
open(os.path.join(HERE, "sym-lib-table"), "w", newline="\n").write(
    f'(sym_lib_table\n\t(version 7)\n\t(lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.kicad_sym")(options "")(descr "Project symbols"))\n)\n')
print("wrote difftxlarge schematic; root uuid", ROOT_UUID, "symbols:", len([i for i in items if i.startswith("\t(symbol")]))
