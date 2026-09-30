"""Trace every output from its Pi header pin to its RJ45 pins in the exported netlist, and check the power nets.

  python check_netlist.py      (after: kicad-cli sch export netlist -o difftxlarge.net difftxlarge.kicad_sch)
"""
import re, sys
t = open("difftxlarge.net", encoding="utf-8").read()
nets = {}
for blk in re.split(r'\n\t\t\(net\n', t[t.find("(nets"):])[1:]:
    name = re.search(r'\(name "([^"]*)"', blk).group(1).lstrip("/")
    nets[name] = set(re.findall(r'\(ref "([^"]+)"\)\s*\(pin "([^"]+)"\)', blk))
where = {node: n for n, s in nets.items() for node in s}
def net(ref, pin):
    return where.get((ref, str(pin)))
def other(res_node):
    ref, pin = res_node
    return net(ref, "2" if pin == "1" else "1")

HDR = {4: 7, 5: 29, 6: 31, 7: 26, 8: 24, 9: 21, 10: 19, 11: 23, 12: 32, 13: 33, 14: 8, 15: 10, 16: 36, 17: 11,
       18: 12, 19: 35, 20: 38, 21: 40, 22: 15, 23: 16, 25: 22, 26: 37, 27: 13}
CH = [("15", "14", "13", "1", "2"), ("1", "2", "3", "3", "6"), ("7", "6", "5", "5", "4"), ("9", "10", "11", "7", "8")]
LE = [27, 26, 25]
BUFS = ("U25", "U26", "U27")

def pi_to_bus(gpio):
    """Pi header pin -> pull-down present -> buffer A -> buffer Y -> 33 R -> bus net."""
    pn = net("J16", HDR[gpio])
    assert pn, gpio
    assert any(r.startswith("RP") for r, _ in nets[pn]), f"no pull-down on GPIO{gpio}"
    (ub, ap), = [x for x in nets[pn] if x[0] in BUFS]
    yb = net(ub, 20 - int(ap))                      # AHCT541: A(n) pin n+1 -> Y(n) pin 19-n
    (rs,) = [x for x in nets[yb] if x[0].startswith("RS")]
    return other(rs)

err = 0
for k in range(60):
    b, i = divmod(k, 20); j, p = divmod(k, 4); m, bit = divmod(i, 8)
    q = 4 * (bit // 4) + 3 - bit % 4          # latch position of this bit (see gen_sch.py)
    latch, drv, jack = f"U{16 + 3*b + m}", f"U{j+1}", f"J{j+1}"
    a, yp, zp, rp, rn = CH[p]
    checks = {
        "data line": pi_to_bus(4 + i) == net(latch, 9 - q),
        "latch enable": pi_to_bus(LE[b]) == net(latch, 11),
        "Q to driver": net(latch, 12 + q) == net(drv, a) == net(f"LED{k+1}", 1),
        "+ line": net(drv, yp) == net(jack, rp) == net(f"D{k+1}", 1),
        "- line": net(drv, zp) == net(jack, rn) == net(f"D{k+1}", 2),
        "LED return": net(f"RA{k+1}", 2) == "GND" and net(f"RA{k+1}", 1) == net(f"LED{k+1}", 2),
        "ESD ground": net(f"D{k+1}", 3) == "GND",
    }
    for what, ok in checks.items():
        if not ok:
            err += 1; print(f"output {k+1} (J{j+1}-{p+1}): {what} WRONG")
for n in range(1, 16):
    for pin, want in (("4", "GND"), ("12", "GND"), ("8", "GND"), ("16", "5V_DRV")):
        if net(f"U{n}", pin) != want:
            err += 1; print(f"U{n} pin {pin} = {net(f'U{n}', pin)}, want {want}")
for u in range(16, 25):
    for pin, want in (("1", "GND"), ("10", "GND"), ("20", "5V_DRV")):
        if net(f"U{u}", pin) != want:
            err += 1; print(f"U{u} pin {pin} = {net(f'U{u}', pin)}, want {want}")
for pin in ("2", "4"):
    if net("J16", pin) is not None and not net("J16", pin).startswith("unconnected"):
        err += 1; print("J16 5 V pin", pin, "is connected to", net("J16", pin))
print("nets:", len(nets), " errors:", err)
for n in ("5V_PI", "5V_DRV", "P3V3", "+12V", "GND", "SDA", "SCL", "12VP"):
    print(f"  {n:7s} {len(nets.get(n, ())):4d} pins")
print("  5V_PI:", sorted(nets["5V_PI"]))
sys.exit(1 if err else 0)
