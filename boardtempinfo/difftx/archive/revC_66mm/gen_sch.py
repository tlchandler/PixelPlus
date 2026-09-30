"""
Rev C schematic generator: FPP Remote pHAT for Raspberry Pi Zero 2 W.

  python gen_sch.py

Outputs: difftx.kicad_sch, difftx.kicad_pro, difftx.kicad_sym, fp-lib-table, sym-lib-table

Design (verified against primary sources, see VERIFICATION.md):
  J3  2x20 socket onto the Pi Zero 2 W header (JLC C5124634)
      P1-7 (GPIO4, DPI_D0) -> U2 1A     FPP DPIPixels string pin names from
      P1-26(GPIO7, DPI_D3) -> U2 4A     DPIPixels.cpp GetDPIPinBitPosition()
      P1-29(GPIO5, DPI_D1) -> U2 3A
      P1-31(GPIO6, DPI_D2) -> U2 2A
      P1-3 / P1-5 = SDA1/SCL1 (i2c-1)  -> U3 cape EEPROM at 0x50 (CapeUtils.cpp: PLATFORM_PI I2C_DEV 1)
      P1-1 3V3 -> U3 VCC,  P1-2/4 5V <- U4 output (back-powering per HAT design guide)
  U2  AM26C31IDR quad RS-422 driver, G and ~G tied low (enabled)  (JLC C34923)
  J1  Ckmtw R-RJ45R08P-A004 RJ45, Falcon differential pinout   (JLC C385834)
  U3  AT24C256C-SSHL-T EEPROM, A0-A2 = GND, WP pulled low by R1, JP1 closes WP to 3V3 (JLC C6482)
  U4  K7805-2000R3 12 V -> 5 V 2 A module, pins 1 Vin 2 GND 3 Vout           (JLC C18212380)
  J4  KF301-5.0-2P screw terminal 12 V in, F1 1.5 A polyfuse, D1 SS54 reverse protection
"""
import os, re, uuid, json

HERE = os.path.dirname(os.path.abspath(__file__))
KISYM = r"C:\Program Files\KiCad\10.0\share\kicad\symbols"
PROJECT = "difftx"

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
sym_drv  = rename_symbol(lib_symbol("Interface.kicad_sym", "AM26LS31CD"), "AM26LS31CD", "AM26C31")
sym_drv  = set_prop(sym_drv, "Value", "AM26C31IDR")
sym_drv  = set_prop(sym_drv, "Footprint", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL")
sym_pihdr = rename_symbol(lib_symbol("Connector_Generic.kicad_sym", "Conn_02x20_Odd_Even"), "Conn_02x20_Odd_Even", "PiHeader")
sym_eep  = rename_symbol(lib_symbol("Memory_EEPROM.kicad_sym", "24LC256"), "24LC256", "AT24C256C")
sym_eep  = set_prop(sym_eep, "Value", "AT24C256C-SSHL-T")
sym_reg  = rename_symbol(lib_symbol("Regulator_Linear.kicad_sym", "L7805"), "L7805", "K7805")
sym_reg  = set_prop(sym_reg, "Value", "K7805-2000R3")
sym_fuse = rename_symbol(lib_symbol("Device.kicad_sym", "Polyfuse"), "Polyfuse", "Polyfuse")
sym_term = rename_symbol(lib_symbol("Connector.kicad_sym", "Screw_Terminal_01x02"), "Screw_Terminal_01x02", "Screw_Terminal_01x02")
sym_jp   = rename_symbol(lib_symbol("Jumper.kicad_sym", "SolderJumper_2_Open"), "SolderJumper_2_Open", "SolderJumper_2_Open")
sym_r    = rename_symbol(lib_symbol("Device.kicad_sym", "R"), "R", "R")
sym_c    = rename_symbol(lib_symbol("Device.kicad_sym", "C"), "C", "C")
sym_hole = rename_symbol(lib_symbol("Mechanical.kicad_sym", "MountingHole"), "MountingHole", "MountingHole")
# SS54 from the JLC library: pin 1 = A, pin 2 = K, matching the JLC SMA footprint (cathode bar at pad 2)
jlc_lib = open(os.path.join(HERE, "jlc", "jlc.kicad_sym"), encoding="utf-8").read()
sym_ss54 = rename_symbol(lib_block(jlc_lib, "SS54_C22452"), "SS54_C22452", "SS54")
sym_ss54 = set_prop(sym_ss54, "Value", "SS54")
sym_ss54 = sym_ss54.replace("(pin unspecified", "(pin passive")   # JLC symbols leave pin types unspecified
pw = open(os.path.join(KISYM, "power.kicad_sym"), encoding="utf-8").read()
def power_sym(name):
    return lib_block(pw, name).replace(f'(symbol "{name}"', f'(symbol "power:{name}"', 1)
sym_gnd, sym_5v, sym_3v3, sym_12v, sym_flag = (power_sym(n) for n in ("GND", "+5V", "+3V3", "+12V", "PWR_FLAG"))

def rj45_symbol():
    s = [f'\t(symbol "{PROJECT}:RJ45_8P8C"', '\t\t(pin_names\n\t\t\t(offset 1.016)\n\t\t)',
         '\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(in_pos_files yes)\n\t\t(duplicate_pin_numbers_are_jumpers no)']
    s.append(prop("Reference", "J", 0, 13.97)); s.append(prop("Value", "RJ45_8P8C", 0, -13.97))
    s.append(prop("Footprint", "jlc:RJ45-TH_R-RJ45R08P-A004", 0, -16.51, hide=True))
    s.append(prop("Datasheet", "https://www.lcsc.com/datasheet/C385834.pdf", 0, -19.05, hide=True))
    s.append(prop("Description", "RJ45 8P8C jack, right angle, unshielded (Ckmtw R-RJ45R08P-A004), Falcon differential pinout", 0, -21.59, hide=True))
    s.append('\t\t(symbol "RJ45_8P8C_0_1"\n\t\t\t(rectangle\n\t\t\t\t(start -7.62 11.43)\n\t\t\t\t(end 7.62 -11.43)\n\t\t\t\t(stroke\n\t\t\t\t\t(width 0.254)\n\t\t\t\t\t(type default)\n\t\t\t\t)\n\t\t\t\t(fill\n\t\t\t\t\t(type background)\n\t\t\t\t)\n\t\t\t)\n\t\t)')
    p = ['\t\t(symbol "RJ45_8P8C_1_1"']
    names = {1: "P1+", 2: "P1-", 3: "P2+", 4: "P3+", 5: "P3-", 6: "P2-", 7: "P4+", 8: "P4-"}
    for k in range(1, 9):
        y = 8.89 - 2.54 * (k - 1)
        p.append(f'\t\t\t(pin passive line\n\t\t\t\t(at -10.16 {y:g} 0)\n\t\t\t\t(length 2.54)\n\t\t\t\t(name "{names[k]}"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t\t(number "{k}"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)')
    p.append('\t\t)'); s.append("\n".join(p)); s.append('\t\t(embedded_fonts no)\n\t)')
    return "\n".join(s)
sym_rj = rj45_symbol()

LIB_SYMBOLS = "\n".join(["\t(lib_symbols", sym_drv, sym_pihdr, sym_eep, sym_reg, sym_fuse, sym_term, sym_jp, sym_r, sym_c,
                         sym_hole, sym_ss54, sym_rj, sym_gnd, sym_5v, sym_3v3, sym_12v, sym_flag, "\t)"])

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
        elif c[0] in ("GND", "+5V", "+3V3", "+12V"):
            power(c[0], ex, ey)
            if len(c) > 1 and c[1] == "flag":
                pwr_flag(ex, ey)

# ---- Pi header --------------------------------------------------------------
PI = {"1": ("+3V3", "flag"), "17": ("+3V3",), "2": ("+5V",), "4": ("+5V",),
      "3": ("label", "SDA1"), "5": ("label", "SCL1"),
      "7": ("label", "DPI_D0"), "26": ("label", "DPI_D3"), "29": ("label", "DPI_D1"), "31": ("label", "DPI_D2")}
for gnd in ("6", "9", "14", "20", "25", "30", "34", "39"):
    PI[gnd] = ("GND", "flag") if gnd == "6" else ("GND",)
place(f"{PROJECT}:PiHeader", "J3", "Pi Zero 2 W", f"{PROJECT}:PiSocket_2x20_bottom", 50.8, 88.9, sym_pihdr, PI,
      datasheet="https://datasheets.raspberrypi.com/rpizero2/raspberry-pi-zero-2-w-product-brief.pdf",
      descr="2x20 socket for Raspberry Pi Zero 2 W (board mounts on the Pi as a pHAT). P1-7/26/29/31 = DPI_D0..D3 for FPP DPIPixels",
      lcsc="C5124634")

place(f"{PROJECT}:AM26C31", "U2", "AM26C31IDR", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", 127.0, 88.9, sym_drv, {
    "1": ("label", "DPI_D0"), "15": ("label", "DPI_D3"), "9": ("label", "DPI_D1"), "7": ("label", "DPI_D2"),
    "2": ("label", "P4+"), "3": ("label", "P4-"), "6": ("label", "P1+"), "5": ("label", "P1-"),
    "14": ("label", "P2+"), "13": ("label", "P2-"), "10": ("label", "P3+"), "11": ("label", "P3-"),
    "4": ("GND",), "12": ("GND",), "16": ("+5V",), "8": ("GND",),
}, datasheet="https://www.ti.com/lit/ds/symlink/am26c31.pdf", descr="Quad RS-422 differential line driver, SOIC-16, G and ~G low = enabled", lcsc="C34923")

place(f"{PROJECT}:RJ45_8P8C", "J1", "R-RJ45R08P-A004", "jlc:RJ45-TH_R-RJ45R08P-A004", 175.26, 88.9, sym_rj, {
    "1": ("label", "P1+"), "2": ("label", "P1-"), "3": ("label", "P2+"), "4": ("label", "P3+"),
    "5": ("label", "P3-"), "6": ("label", "P2-"), "7": ("label", "P4+"), "8": ("label", "P4-"),
}, datasheet="https://www.lcsc.com/datasheet/C385834.pdf", descr="RJ45 8P8C jack, right angle, unshielded, Falcon differential pinout", lcsc="C385834")
place(f"{PROJECT}:C", "C1", "100nF", "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", 152.4, 68.58, sym_c,
      {"1": ("+5V",), "2": ("GND",)}, descr="Ceramic bypass 100nF 50V X7R 0805 at AM26C31 VCC", lcsc="C49678")

# ---- EEPROM ---------------------------------------------------------------------
place(f"{PROJECT}:AT24C256C", "U3", "AT24C256C-SSHL-T", "jlc:SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", 127.0, 139.7, sym_eep, {
    "1": ("GND",), "2": ("GND",), "3": ("GND",), "4": ("GND",),
    "5": ("label", "SDA1"), "6": ("label", "SCL1"), "7": ("label", "WP"), "8": ("+3V3",),
}, datasheet="https://ww1.microchip.com/downloads/en/devicedoc/atmel-8568-seeprom-at24c256c-datasheet.pdf",
   descr="FPP cape EEPROM, 32 KB, I2C addr 0x50 (A0-A2 = GND), on Pi i2c-1", lcsc="C6482")
place(f"{PROJECT}:C", "C2", "100nF", "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", 152.4, 134.62, sym_c,
      {"1": ("+3V3",), "2": ("GND",)}, descr="Ceramic bypass 100nF 0805 at EEPROM VCC", lcsc="C49678")
place(f"{PROJECT}:R", "R1", "10k", "Resistor_SMD:R_0603_1608Metric", 101.6, 149.86, sym_r,
      {"1": ("label", "WP"), "2": ("GND",)}, descr="EEPROM WP pull-down: writable (FPP programs and signs the EEPROM in place)", lcsc="C25804")
place(f"{PROJECT}:SolderJumper_2_Open", "JP1", "WP", "Jumper:SolderJumper-2_P1.3mm_Open_Pad1.0x1.5mm", 101.6, 132.08, sym_jp,
      {"1": ("label", "WP"), "2": ("+3V3",)}, descr="Close after signing to write-protect the EEPROM (WP high)")

# ---- 12 V power input ----------------------------------------------------------
place(f"{PROJECT}:Screw_Terminal_01x02", "J4", "12V IN", "jlc:CONN-TH_P5.00_KF301-5.0-2P", 30.48, 152.4, sym_term,
      {"1": ("label", "12V_IN"), "2": ("GND",)}, descr="12 V DC input, 5.08 mm screw terminal (pin 1 = +12 V, pin 2 = GND)", lcsc="C474881")
place(f"{PROJECT}:Polyfuse", "F1", "1.5A", "jlc:F1812", 50.8, 147.32, sym_fuse,
      {"1": ("label", "12V_IN"), "2": ("label", "12V_F")}, descr="Resettable fuse 1.5 A hold / 3 A trip, 24 V, 1812", lcsc="C22392774")
place(f"{PROJECT}:SS54", "D1", "SS54", "jlc:SMA_L4.4-W2.8-LS5.4-R-RD", 66.04, 147.32, sym_ss54,
      {"1": ("label", "12V_F"), "2": ("+12V", "flag")}, descr="Reverse polarity protection, 40 V 5 A Schottky, SMA", lcsc="C22452")
place(f"{PROJECT}:K7805", "U4", "K7805-2000R3", "jlc:PWRM-TH_K78XX-2000R3", 91.44, 147.32, sym_reg,
      {"1": ("+12V",), "2": ("GND",), "3": ("+5V",)},
      datasheet="https://www.lcsc.com/datasheet/C18212380.pdf", descr="12 V -> 5 V 2 A switching regulator module, 78xx pinout (1 Vin, 2 GND, 3 Vout)", lcsc="C18212380")
for i, x in enumerate((60.96, 68.58, 76.2)):
    place(f"{PROJECT}:C", f"C{3+i}", "10uF", "Capacitor_SMD:C_1206_3216Metric", x, 165.1, sym_c,
          {"1": ("+12V",), "2": ("GND",)}, descr="Input cap 10uF 50V X7R 1206 (datasheet asks 22uF ceramic at Vin; three in parallel)", lcsc="C89632")
place(f"{PROJECT}:C", "C6", "22uF", "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", 106.68, 165.1, sym_c,
      {"1": ("+5V",), "2": ("GND",)}, descr="Output cap 22uF 25V X5R 0805 (datasheet: 22uF at Vout)", lcsc="C45783")

HOLES = [("MountingHole:MountingHole_2.7mm_M2.5", "M2.5", "Mounting hole M2.5, Raspberry Pi Zero pattern (58 x 23 mm)")] * 4 + \
        [("MountingHole:MountingHole_3.2mm_M3", "M3", "Mounting hole M3")] * 2
for i, (fpn, val, d) in enumerate(HOLES):
    place(f"{PROJECT}:MountingHole", f"H{i+1}", val, fpn, 30.48 + 12.7 * i, 185.42, sym_hole, {}, descr=d)

text("FPP Remote pHAT for Raspberry Pi Zero 2 W: 4-port RS-422 (Falcon differential) pixel driver, rev C", 20, 18, 2.5)
text("Pi mounts under this board on 11 mm standoffs (M2.5 holes H1-H4, 58 x 23 mm). Power: 12 V in on J4 -> F1 -> D1 -> U4 -> 5 V onto Pi pins 2/4.", 20, 23, 1.5)
text("NEVER power the Pi from its USB while 12 V is connected (a Pi Zero has no diode between its USB 5 V input and the header 5 V pins).", 20, 27, 1.5)
text("FPP: cape EEPROM U3 on i2c-1 @0x50 (fppcapedetect registers a 24c256). DPIPixels strings: port1=P1-31, port2=P1-26, port3=P1-29, port4=P1-7.", 20, 31, 1.5)
text("RJ45: port1 = pins 1(+),2(-)  port2 = 3(+),6(-)  port3 = 4(+),5(-)  port4 = 7(+),8(-).  Y = +, Z = -.  Driver enables G and ~G tied low.", 20, 35, 1.5)
text("JP1 open (default): EEPROM writable so FPP can program and sign it in place. Close JP1 afterwards to write-protect (WP high).", 20, 39, 1.5)

sch = "\n".join(["(kicad_sch", "\t(version 20260306)", '\t(generator "eeschema")', '\t(generator_version "10.0")',
                 f'\t(uuid "{ROOT_UUID}")', '\t(paper "A3")',
                 '\t(title_block\n\t\t(title "FPP Remote pHAT, 4-port RS-422 pixel driver")\n\t\t(date "2026-09-19")\n\t\t(rev "C")\n\t)',
                 LIB_SYMBOLS, *items, '\t(sheet_instances\n\t\t(path "/"\n\t\t\t(page "1")\n\t\t)\n\t)', '\t(embedded_fonts no)', ")"])
open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), "w", encoding="utf-8", newline="\n").write(sch)

lib = '(kicad_symbol_lib\n\t(version 20251024)\n\t(generator "kicad_symbol_editor")\n\t(generator_version "10.0")\n'
for b in [sym_drv, sym_pihdr, sym_eep, sym_reg, sym_fuse, sym_term, sym_jp, sym_r, sym_c, sym_hole, sym_ss54, sym_rj]:
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
open(os.path.join(HERE, f"{PROJECT}.kicad_pro"), "w", encoding="utf-8", newline="\n").write(json.dumps(pro, indent=2))
open(os.path.join(HERE, "fp-lib-table"), "w", newline="\n").write(
    '(fp_lib_table\n\t(version 7)\n\t(lib (name "jlc")(type "KiCad")(uri "${KIPRJMOD}/jlc/jlc.pretty")(options "")(descr "JLCPCB footprints via easyeda2kicad"))\n\t(lib (name "difftx")(type "KiCad")(uri "${KIPRJMOD}/difftx.pretty")(options "")(descr "project footprints"))\n)\n')
open(os.path.join(HERE, "sym-lib-table"), "w", newline="\n").write(
    f'(sym_lib_table\n\t(version 7)\n\t(lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.kicad_sym")(options "")(descr "Project symbols"))\n)\n')
print("wrote rev C schematic; root uuid", ROOT_UUID)
