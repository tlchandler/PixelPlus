"""
Generate the KiCad 10 schematic, project, custom footprint library and 3D model
for the Pico 2 W -> AM26C31 -> CN0035 RJ45 differential driver board.

Run with any Python 3:  python gen_sch.py
Outputs (next to this script):
  difftx.kicad_sch, difftx.kicad_pro, difftx.kicad_sym,
  fp-lib-table, sym-lib-table,
  difftx.pretty/CN0035.kicad_mod, difftx.3dshapes/CN0035.wrl
"""
import os, re, uuid, json

HERE = os.path.dirname(os.path.abspath(__file__))
KISYM = r"C:\Program Files\KiCad\10.0\share\kicad\symbols"
PROJECT = "difftx"

# ----------------------------------------------------------------------
# helpers
# ----------------------------------------------------------------------
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

def lib_symbol(libfile, name):
    t = open(os.path.join(KISYM, libfile), encoding="utf-8").read()
    i = t.find(f'(symbol "{name}"\n')
    if i < 0:
        i = t.find(f'(symbol "{name}"')
    assert i >= 0, (libfile, name)
    return block_at(t, i)

def rename_symbol(block, oldname, newname):
    """Rename a library symbol block (and its sub-units) to difftx:newname."""
    block = block.replace(f'(symbol "{oldname}"', f'(symbol "{PROJECT}:{newname}"', 1)
    block = re.sub(r'\(symbol "' + re.escape(oldname) + r'_(\d+_\d+)"',
                   lambda m: f'(symbol "{newname}_{m.group(1)}"', block)
    return block

def set_prop(block, prop, value):
    return re.sub(r'\(property "' + prop + r'" "[^"]*"',
                  f'(property "{prop}" "{value}"', block, count=1)

PIN_RE = re.compile(r'\(pin (\w+) \w+\s*\(at ([-\d.]+) ([-\d.]+) (\d+)\)\s*\(length ([\d.]+)\).*?\(name "([^"]*)".*?\(number "([^"]*)"', re.S)

def pins_of(block):
    """number -> (x, y, angle, name, type) in symbol coordinates (y up)."""
    out = {}
    for m in PIN_RE.finditer(block):
        out.setdefault(m.group(7), (float(m.group(2)), float(m.group(3)), int(m.group(4)), m.group(6), m.group(1)))
    return out

# ----------------------------------------------------------------------
# library symbols -> our embedded library "difftx"
# ----------------------------------------------------------------------
sym_pico = rename_symbol(lib_symbol("MCU_Module.kicad_sym", "RaspberryPi_Pico"), "RaspberryPi_Pico", "Pico2W")
sym_pico = set_prop(sym_pico, "Value", "Pico2W")
sym_pico = set_prop(sym_pico, "Footprint", "jlc:COMM-SMD_L51.0-W21.0-P2.54_PICOW")
sym_pico = set_prop(sym_pico, "Datasheet", "https://datasheets.raspberrypi.com/picow/pico-2-w-datasheet.pdf")
sym_pico = set_prop(sym_pico, "Description", "Raspberry Pi Pico 2 W module, RP2350, 2.4 GHz wireless, reflowed on castellations")

sym_drv = rename_symbol(lib_symbol("Interface.kicad_sym", "AM26LS31CD"), "AM26LS31CD", "AM26C31")
sym_drv = set_prop(sym_drv, "Value", "AM26C31IDR")
sym_drv = set_prop(sym_drv, "Footprint", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL")
sym_drv = set_prop(sym_drv, "Datasheet", "https://www.ti.com/lit/ds/symlink/am26c31.pdf")
sym_drv = set_prop(sym_drv, "Description", "Quad differential line driver, RS-422 (TIA/EIA-422-B), 3-state, SOIC-16")

sym_c    = rename_symbol(lib_symbol("Device.kicad_sym", "C"), "C", "C")
sym_cp   = rename_symbol(lib_symbol("Device.kicad_sym", "C_Polarized"), "C_Polarized", "C_Polarized")
sym_hdr  = rename_symbol(lib_symbol("Connector_Generic.kicad_sym", "Conn_01x02"), "Conn_01x02", "Conn_01x02")
sym_hole = rename_symbol(lib_symbol("Mechanical.kicad_sym", "MountingHole"), "MountingHole", "MountingHole")
sym_gnd  = lib_symbol("power.kicad_sym", "GND")      # keep power: prefix
sym_5v   = lib_symbol("power.kicad_sym", "+5V")
sym_gnd  = sym_gnd.replace('(symbol "GND"', '(symbol "power:GND"', 1)
sym_5v   = sym_5v.replace('(symbol "+5V"', '(symbol "power:+5V"', 1)

def prop(name, value, x, y, hide=False, justify=None):
    j = f"\n\t\t\t\t(justify {justify})" if justify else ""
    h = "\n\t\t\t(hide yes)" if hide else ""
    return (f'\t\t(property "{name}" "{value}"\n\t\t\t(at {x} {y} 0)\n\t\t\t(show_name no)\n\t\t\t(do_not_autoplace no){h}'
            f'\n\t\t\t(effects\n\t\t\t\t(font\n\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t){j}\n\t\t\t)\n\t\t)\n')

# Custom CN0035 symbol: 8 pins on the left, body rectangle.
def cn0035_symbol():
    s = [f'\t(symbol "{PROJECT}:CN0035"',
         '\t\t(pin_names\n\t\t\t(offset 1.016)\n\t\t)',
         '\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(in_pos_files yes)\n\t\t(duplicate_pin_numbers_are_jumpers no)']
    s.append(prop("Reference", "J", 0, 13.97).replace("\t\t", "\t\t", 1))
    s.append(prop("Value", "CN0035", 0, -13.97))
    s.append(prop("Footprint", f"{PROJECT}:CN0035", 0, -16.51, hide=True))
    s.append(prop("Datasheet", "https://www.digikey.com/en/products/detail/chip-quik-inc/CN0035/5978221", 0, -19.05, hide=True))
    s.append(prop("Description", "Chip Quik CN0035 RJ45 8P8C adapter board (EDAC A00-108-220-450 top-entry jack) on 8-pin SIP header", 0, -21.59, hide=True))
    s.append(prop("ki_keywords", "RJ45 8P8C ethernet jack breakout SIP", 0, 0, hide=True))
    s.append(prop("ki_fp_filters", "CN0035*", 0, 0, hide=True))
    s.append('\t\t(symbol "CN0035_0_1"\n\t\t\t(rectangle\n\t\t\t\t(start -7.62 11.43)\n\t\t\t\t(end 7.62 -11.43)\n\t\t\t\t(stroke\n\t\t\t\t\t(width 0.254)\n\t\t\t\t\t(type default)\n\t\t\t\t)\n\t\t\t\t(fill\n\t\t\t\t\t(type background)\n\t\t\t\t)\n\t\t\t)\n'
             '\t\t\t(text "RJ45"\n\t\t\t\t(at 1.27 6.35 0)\n\t\t\t\t(effects\n\t\t\t\t\t(font\n\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)\n\t\t)')
    p = ['\t\t(symbol "CN0035_1_1"']
    names = {1: "P1+", 2: "P1-", 3: "P2+", 4: "P3+", 5: "P3-", 6: "P2-", 7: "P4+", 8: "P4-"}
    for k in range(1, 9):
        y = 8.89 - 2.54 * (k - 1)
        p.append(f'\t\t\t(pin passive line\n\t\t\t\t(at -10.16 {y:g} 0)\n\t\t\t\t(length 2.54)\n\t\t\t\t(name "{names[k]}"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t\t(number "{k}"\n\t\t\t\t\t(effects\n\t\t\t\t\t\t(font\n\t\t\t\t\t\t\t(size 1.27 1.27)\n\t\t\t\t\t\t)\n\t\t\t\t\t)\n\t\t\t\t)\n\t\t\t)')
    p.append('\t\t)')
    s.append("\n".join(p))
    s.append('\t\t(embedded_fonts no)\n\t)')
    return "\n".join(s)

sym_rj = cn0035_symbol()

LIB_SYMBOLS = "\n".join(["\t(lib_symbols", sym_pico, sym_drv, sym_rj, sym_c, sym_cp, sym_hdr, sym_hole, sym_gnd, sym_5v, "\t)"])

# ----------------------------------------------------------------------
# schematic content
# ----------------------------------------------------------------------
ROOT_UUID = U()
items = []          # top-level s-expressions
pwr_count = [0]

def sym_instance(lib_id, ref, value, footprint, x, y, pins, extra_props=None, rot=0, datasheet="", descr=""):
    lines = [f'\t(symbol\n\t\t(lib_id "{lib_id}")\n\t\t(at {x:g} {y:g} {rot})\n\t\t(unit 1)\n\t\t(body_style 1)\n\t\t(exclude_from_sim no)\n\t\t(in_bom yes)\n\t\t(on_board yes)\n\t\t(in_pos_files yes)\n\t\t(dnp no)\n\t\t(uuid "{U()}")']
    hide_ref = ref.startswith("#")
    lines.append(prop("Reference", ref, x, y - 2.54, hide=hide_ref))
    lines.append(prop("Value", value, x, y + 2.54))
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
    ref = f"#PWR{pwr_count[0]:02d}"
    lib = "power:GND" if kind == "GND" else "power:+5V"
    sym_instance(lib, ref, kind, "", x, y, ["1"],
                 descr=f'Power symbol creates a global label with name {kind}')

OUT = {0: (-1, 0), 180: (1, 0), 90: (0, 1), 270: (0, -1)}   # outward direction in schematic coords

def place(lib_id, ref, value, footprint, x, y, block, conn, datasheet="", descr="", lcsc=""):
    """Place a symbol at (x,y) and connect its pins.
    conn: pin number -> ("label", name) | ("GND",) | ("+5V",) | ("NC",)
    Pins not listed get a no-connect flag."""
    pins = pins_of(block)
    sym_instance(lib_id, ref, value, footprint, x, y, list(pins.keys()), datasheet=datasheet, descr=descr,
                 extra_props={"LCSC": lcsc} if lcsc else None)
    done_xy = set()
    for pn, (px, py, ang, name, ptype) in pins.items():
        cx, cy = x + px, y - py
        c = conn.get(pn, ("NC",))
        key = (round(cx, 3), round(cy, 3))
        if key in done_xy:          # stacked pins (Pico GND) share one wire
            continue
        done_xy.add(key)
        dx, dy = OUT[ang]
        ex, ey = cx + dx * 2.54, cy + dy * 2.54
        if c[0] == "NC":
            no_connect(cx, cy)
            continue
        wire(cx, cy, ex, ey)
        if c[0] == "label":
            if dx < 0:
                label(c[1], ex, ey, 180, "right bottom")
            elif dx > 0:
                label(c[1], ex, ey, 0, "left bottom")
            elif dy < 0:
                label(c[1], ex, ey, 90, "left bottom")
            else:
                label(c[1], ex, ey, 270, "right bottom")
        elif c[0] == "GND":
            power("GND", ex, ey)
        elif c[0] == "+5V":
            power("+5V", ex, ey)

# ---- placements (schematic mm, y down) --------------------------------
PICO_X, PICO_Y = 63.5, 88.9
place(f"{PROJECT}:Pico2W", "U1", "Pico2W", "jlc:COMM-SMD_L51.0-W21.0-P2.54_PICOW", PICO_X, PICO_Y, sym_pico, {
    "4": ("label", "GP2"), "5": ("label", "GP3"), "6": ("label", "GP4"), "7": ("label", "GP5"),
    "20": ("label", "GP15"),
    "3": ("GND",), "8": ("GND",), "13": ("GND",), "18": ("GND",), "23": ("GND",), "28": ("GND",), "38": ("GND",),
    "40": ("+5V",),
}, datasheet="https://datasheets.raspberrypi.com/picow/pico-2-w-datasheet.pdf", descr="Raspberry Pi Pico 2 W", lcsc="C42394205")

DRV_X, DRV_Y = 139.7, 88.9
place(f"{PROJECT}:AM26C31", "U2", "AM26C31IDR", "jlc:SOIC-16_L9.9-W3.9-P1.27-LS6.0-BL", DRV_X, DRV_Y, sym_drv, {
    "1": ("label", "GP2"), "15": ("label", "GP3"), "9": ("label", "GP4"), "7": ("label", "GP5"),
    "2": ("label", "P4+"), "3": ("label", "P4-"),
    "6": ("label", "P1+"), "5": ("label", "P1-"),
    "14": ("label", "P2+"), "13": ("label", "P2-"),
    "10": ("label", "P3+"), "11": ("label", "P3-"),
    "4": ("GND",), "12": ("GND",), "16": ("+5V",), "8": ("GND",),
}, datasheet="https://www.ti.com/lit/ds/symlink/am26c31.pdf", descr="Quad RS-422 differential line driver, SOIC-16", lcsc="C34923")

RJ_X, RJ_Y = 190.5, 88.9
place(f"{PROJECT}:CN0035", "J1", "R-RJ45R08P-A004", "jlc:RJ45-TH_R-RJ45R08P-A004", RJ_X, RJ_Y, sym_rj, {
    "1": ("label", "P1+"), "2": ("label", "P1-"), "3": ("label", "P2+"), "4": ("label", "P3+"),
    "5": ("label", "P3-"), "6": ("label", "P2-"), "7": ("label", "P4+"), "8": ("label", "P4-"),
}, datasheet="https://www.lcsc.com/datasheet/C385834.pdf", descr="RJ45 8P8C jack, right angle, unshielded, Falcon differential pinout", lcsc="C385834")

place(f"{PROJECT}:C", "C1", "100nF", "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", 165.1, 44.45, sym_c,
      {"1": ("+5V",), "2": ("GND",)}, descr="Ceramic bypass 100nF 50V X7R 0805, at AM26C31 VCC", lcsc="C49678")
place(f"{PROJECT}:C", "C2", "10uF", "Capacitor_SMD:C_0805_2012Metric_Pad1.18x1.45mm_HandSolder", 180.34, 44.45, sym_c,
      {"1": ("+5V",), "2": ("GND",)}, descr="Ceramic bulk 10uF 25V X5R 0805, 5 V rail", lcsc="C15850")
place(f"{PROJECT}:Conn_01x02", "J2", "SCOPE", "jlc:HDR-TH_2P-P2.54-V-M-3", 114.3, 139.7, sym_hdr,
      {"1": ("label", "GP15"), "2": ("GND",)}, descr="Scope test point: GP15 frame toggle + GND, 1x2 pin header", lcsc="C32713268")
for i, (hx) in enumerate([63.5, 76.2, 88.9, 101.6]):
    place(f"{PROJECT}:MountingHole", f"H{i+1}", "M3", "MountingHole:MountingHole_3.2mm_M3", hx, 165.1, sym_hole, {}, descr="Mounting hole, 3.2 mm, M3")

text("Pico 2 W -> AM26C31 RS-422 driver -> CN0035 RJ45  (Falcon differential receiver, dumb mode)", 30, 20, 2.5)
text("Power: USB into the Pico only. VBUS (pin 40) feeds the 5 V net; AM26C31 G and ~G both tied low (~G low = always enabled).", 30, 25, 1.5)
text("RJ45 pairs: port1 = pins 1(+),2(-)  port2 = 3(+),6(-)  port3 = 4(+),5(-)  port4 = 7(+),8(-).  Y = +, Z = -.", 30, 29, 1.5)
text("Driver map: GP5->2A->port1, GP3->4A->port2, GP4->3A->port3, GP2->1A->port4  (main.py DATA_PINS = (5, 3, 4, 2)).", 30, 33, 1.5)
text("Rev B for JLCPCB assembly: U1 Pico 2W C42394205 | U2 AM26C31IDR C34923 | J1 RJ45 C385834 | C1 C49678 | C2 C15850 | J2 C32713268 | H1-H4 M3", 30, 37, 1.5)

# ----------------------------------------------------------------------
# write files
# ----------------------------------------------------------------------
sch = "\n".join([
    "(kicad_sch",
    "\t(version 20260306)",
    '\t(generator "eeschema")',
    '\t(generator_version "10.0")',
    f'\t(uuid "{ROOT_UUID}")',
    '\t(paper "A3")',
    '\t(title_block\n\t\t(title "Pico 2 W RS-422 pixel driver")\n\t\t(date "2026-09-18")\n\t\t(rev "B")\n\t)',
    LIB_SYMBOLS,
    *items,
    f'\t(sheet_instances\n\t\t(path "/"\n\t\t\t(page "1")\n\t\t)\n\t)',
    '\t(embedded_fonts no)',
    ")",
])
open(os.path.join(HERE, f"{PROJECT}.kicad_sch"), "w", encoding="utf-8", newline="\n").write(sch)

# standalone symbol library (same symbols, no lib prefix) for the GUI
lib = "(kicad_symbol_lib\n\t(version 20251024)\n\t(generator \"kicad_symbol_editor\")\n\t(generator_version \"10.0\")\n"
for b in [sym_pico, sym_drv, sym_rj, sym_c, sym_cp, sym_hdr, sym_hole]:
    lib += b.replace(f'(symbol "{PROJECT}:', '(symbol "', 1) + "\n"
lib += ")\n"
open(os.path.join(HERE, f"{PROJECT}.kicad_sym"), "w", encoding="utf-8", newline="\n").write(lib)

pro = {
    "board": {"design_settings": {"defaults": {}, "diff_pair_dimensions": [], "drc_exclusions": [], "rules": {}, "track_widths": [], "via_dimensions": []}},
    "boards": [], "cvpcb": {"equivalence_files": []},
    "libraries": {"pinned_footprint_libs": [], "pinned_symbol_libs": []},
    "meta": {"filename": f"{PROJECT}.kicad_pro", "version": 3},
    "net_settings": {"classes": [{"name": "Default", "clearance": 0.2, "track_width": 0.25, "via_diameter": 0.8, "via_drill": 0.4,
                                  "bus_width": 12, "wire_width": 6, "line_style": 0, "microvia_diameter": 0.3, "microvia_drill": 0.1,
                                  "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25, "diff_pair_width": 0.2, "pcb_color": "rgba(0, 0, 0, 0.000)",
                                  "schematic_color": "rgba(0, 0, 0, 0.000)", "priority": 2147483647}],
                     "meta": {"version": 4}, "net_colors": None, "netclass_assignments": None, "netclass_patterns": []},
    "pcbnew": {"page_layout_descr_file": ""},
    "schematic": {"legacy_lib_dir": "", "legacy_lib_list": []},
    "sheets": [[ROOT_UUID, "Root"]],
    "text_variables": {},
}
open(os.path.join(HERE, f"{PROJECT}.kicad_pro"), "w", encoding="utf-8", newline="\n").write(json.dumps(pro, indent=2))

open(os.path.join(HERE, "fp-lib-table"), "w", newline="\n").write(
    f'(fp_lib_table\n\t(version 7)\n\t(lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.pretty")(options "")(descr "Project footprints"))\n\t(lib (name "jlc")(type "KiCad")(uri "${{KIPRJMOD}}/jlc/jlc.pretty")(options "")(descr "JLCPCB footprints via easyeda2kicad"))\n)\n')
open(os.path.join(HERE, "sym-lib-table"), "w", newline="\n").write(
    f'(sym_lib_table\n\t(version 7)\n\t(lib (name "{PROJECT}")(type "KiCad")(uri "${{KIPRJMOD}}/{PROJECT}.kicad_sym")(options "")(descr "Project symbols"))\n)\n')

# ---- custom footprint: CN0035 adapter on 8-pin SIP, 2.54 mm ------------
# Pin 1 at origin, pins along +Y. Adapter PCB (20.32 x 17.78) extends to -X,
# the top-entry jack sits on it 2.54 mm above this board.
os.makedirs(os.path.join(HERE, f"{PROJECT}.pretty"), exist_ok=True)
os.makedirs(os.path.join(HERE, f"{PROJECT}.3dshapes"), exist_ok=True)

def fpline(layer, x1, y1, x2, y2, w=0.12):
    return f'\t(fp_line\n\t\t(start {x1:g} {y1:g})\n\t\t(end {x2:g} {y2:g})\n\t\t(stroke\n\t\t\t(width {w})\n\t\t\t(type solid)\n\t\t)\n\t\t(layer "{layer}")\n\t\t(uuid "{U()}")\n\t)'

def fprect(layer, x1, y1, x2, y2, w=0.12):
    return f'\t(fp_rect\n\t\t(start {x1:g} {y1:g})\n\t\t(end {x2:g} {y2:g})\n\t\t(stroke\n\t\t\t(width {w})\n\t\t\t(type solid)\n\t\t)\n\t\t(fill no)\n\t\t(layer "{layer}")\n\t\t(uuid "{U()}")\n\t)'

def fptext(kind, s, x, y, layer, size=1.0, hide=False):
    h = "\n\t\t(hide yes)" if hide else ""
    return f'\t(fp_text {kind} "{s}"\n\t\t(at {x:g} {y:g} 0)\n\t\t(layer "{layer}"){h}\n\t\t(uuid "{U()}")\n\t\t(effects\n\t\t\t(font\n\t\t\t\t(size {size} {size})\n\t\t\t\t(thickness 0.15)\n\t\t\t)\n\t\t)\n\t)'

fp = [f'(footprint "CN0035"', '\t(version 20260206)', '\t(generator "pcbnew")', '\t(generator_version "10.0")', '\t(layer "F.Cu")',
      '\t(descr "Chip Quik CN0035: RJ45 8P8C adapter board (EDAC A00-108-220-450 top entry jack) on 1x8 SIP header, 2.54 mm pitch. Body extends to -X from the pin row.")',
      '\t(tags "RJ45 8P8C adapter SIP")', '\t(attr through_hole)']
fp.append(fptext("reference", "REF**", -7.62, -2.9, "F.SilkS"))
fp.append(fptext("value", "CN0035", -7.62, 20.6, "F.Fab"))
# adapter PCB outline (fab + silk), pin row at x=0
fp.append(fprect("F.Fab", -16.51, -1.27, 1.27, 19.05, 0.1))
fp.append(fprect("F.SilkS", -16.63, -1.39, 1.39, 19.17, 0.12))
# jack body on the adapter (fab only) and plug opening marker
fp.append(fprect("F.Fab", -15.3, 0.7, 0.06, 17.08, 0.1))
fp.append(fprect("F.Fab", -13.5, 4.5, -1.9, 13.3, 0.1))
fp.append(fptext("user", "RJ45 top entry", -7.62, 8.89, "F.Fab", 0.8))
fp.append(fptext("user", "${REFERENCE}", -7.62, 15.5, "F.Fab", 0.8))
# pin 1 marker
fp.append(fpline("F.SilkS", 1.9, -1.27, 1.9, 1.27, 0.15))
fp.append(fpline("F.SilkS", 1.9, -1.27, 2.9, -1.27, 0.15))
fp.append(fptext("user", "1", 3.4, 0, "F.SilkS", 0.8))
fp.append(fprect("F.CrtYd", -17.0, -1.77, 1.77, 19.55, 0.05))
for k in range(8):
    shape = "rect" if k == 0 else "circle"
    fp.append(f'\t(pad "{k+1}" thru_hole {shape}\n\t\t(at 0 {2.54*k:g})\n\t\t(size 1.7 1.7)\n\t\t(drill 1)\n\t\t(layers "*.Cu" "*.Mask")\n\t\t(remove_unused_layers no)\n\t\t(keep_end_layers no)\n\t\t(uuid "{U()}")\n\t)')
fp.append(f'\t(model "${{KIPRJMOD}}/{PROJECT}.3dshapes/CN0035.wrl"\n\t\t(offset\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t\t(scale\n\t\t\t(xyz 1 1 1)\n\t\t)\n\t\t(rotate\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t)')
fp.append('\t(embedded_fonts no)')
fp.append(')')
open(os.path.join(HERE, f"{PROJECT}.pretty", "CN0035.kicad_mod"), "w", encoding="utf-8", newline="\n").write("\n".join(fp) + "\n")

# ---- 3D model (VRML, 1 unit = 2.54 mm as in the KiCad libraries) ------
def box(cx, cy, cz, sx, sy, sz, rgb):
    s = 1 / 2.54
    return (f"Transform {{ translation {cx*s:.4f} {-cy*s:.4f} {cz*s:.4f} children [ Shape {{ appearance Appearance {{ material Material {{ diffuseColor {rgb} }} }} "
            f"geometry Box {{ size {sx*s:.4f} {sy*s:.4f} {sz*s:.4f} }} }} ] }}\n")
# note: KiCad footprint Y axis points down, VRML Y points up -> negate Y
wrl = "#VRML V2.0 utf8\n# CN0035 RJ45 adapter on SIP header, approximate model\n"
wrl += box(0, 8.89, 1.27, 2.54, 20.32, 2.54, "0.1 0.1 0.1")             # header plastic
wrl += box(-7.62, 8.89, 3.34, 17.78, 20.32, 1.6, "0.05 0.45 0.15")        # adapter PCB (green)
wrl += box(-7.68, 8.89, 12.08, 15.36, 16.38, 15.88, "0.08 0.08 0.08")     # EDAC jack body
wrl += box(-7.68, 8.89, 20.05, 11.7, 8.0, 0.2, "0.02 0.02 0.02")          # plug opening (decoration)
for k in range(8):                                                       # pin tips above the adapter
    wrl += box(0, 2.54*k, 4.5, 0.64, 0.64, 0.7, "0.8 0.7 0.3")
open(os.path.join(HERE, f"{PROJECT}.3dshapes", "CN0035.wrl"), "w", newline="\n").write(wrl)

print("wrote", PROJECT, "schematic/project/library/footprint/model; root uuid", ROOT_UUID)
