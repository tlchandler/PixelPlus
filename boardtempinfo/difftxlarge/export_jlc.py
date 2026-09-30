"""
Export JLCPCB assembly files from difftxlarge.kicad_pcb (4-layer board, so the two inner layers go in the zip too). Run with KiCad 10's Python:

  "C:\Program Files\KiCad\10.0\bin\python.exe" export_jlc.py

Writes into jlcpcb/:
  difftxlarge-gerbers.zip     gerbers + Excellon drill (upload as the PCB file)
  difftxlarge-bom.csv         Comment, Designator, Footprint, LCSC Part #
  difftxlarge-cpl.csv         Designator, Mid X, Mid Y, Layer, Rotation
"""
import os, csv, zipfile, subprocess, glob
import pcbnew

HERE = os.path.dirname(os.path.abspath(__file__))
CLI = r"C:\Program Files\KiCad\10.0\bin\kicad-cli.exe"
OUT = os.path.join(HERE, "jlcpcb")
os.makedirs(OUT, exist_ok=True)
pcb = os.path.join(HERE, "difftxlarge.kicad_pcb")

# --- gerbers + drill -> zip -------------------------------------------------
gdir = os.path.join(OUT, "gerber_tmp")
os.makedirs(gdir, exist_ok=True)
for f in glob.glob(os.path.join(gdir, "*")):
    os.remove(f)
subprocess.run([CLI, "pcb", "export", "gerbers", pcb, "-o", gdir + os.sep,
                "--layers", "F.Cu,In1.Cu,In2.Cu,B.Cu,F.Mask,B.Mask,F.SilkS,B.SilkS,F.Paste,B.Paste,Edge.Cuts",
                "--subtract-soldermask", "--use-drill-file-origin"], check=True, capture_output=True)
subprocess.run([CLI, "pcb", "export", "drill", pcb, "-o", gdir + os.sep, "--format", "excellon",
                "--excellon-units", "mm", "--drill-origin", "plot", "--generate-map", "--map-format", "gerberx2"],
               check=True, capture_output=True)
with zipfile.ZipFile(os.path.join(OUT, "difftxlarge-gerbers.zip"), "w", zipfile.ZIP_DEFLATED) as z:
    for f in sorted(glob.glob(os.path.join(gdir, "*"))):
        z.write(f, os.path.basename(f))

# --- BOM and CPL from the board ------------------------------------------------
board = pcbnew.LoadBoard(pcb)
H_mm = None
aux = board.GetDesignSettings().GetAuxOrigin()   # the board's aux origin (bottom-left corner) is also the gerber/drill origin
class _BB:
    def GetLeft(self): return aux.x
    def GetBottom(self): return aux.y
bb = _BB()
rows, groups, comments = [], {}, {}
for fp in board.GetFootprints():
    if fp.IsExcludedFromBOM() or fp.IsExcludedFromPosFiles():
        continue
    ref = fp.GetReference()
    lcsc = ""
    for f in fp.GetFields():
        if f.GetName() == "LCSC":
            lcsc = f.GetText()
    fpname = fp.GetFPIDAsString().split(":")[-1]
    # Group on the LCSC code, not the value: the twelve output terminals are one part number with
    # twelve different port names in their Value field, and twelve rows of the same part number with
    # mismatched comments is what triggers a JLCPCB DFM query.
    key = (lcsc or fp.GetValue(), fpname)
    groups.setdefault(key, []).append(ref)
    # only keep a shared value as the comment; twelve terminals named after twelve different ports
    # must not end up labelled with whichever one was seen first
    if key in comments and comments[key] != fp.GetValue():
        comments[key] = fpname.split("_")[0]
    else:
        comments.setdefault(key, fp.GetValue())
    pos = fp.GetPosition()
    # JLC: origin bottom-left of the board, Y up, mm
    x = (pos.x - bb.GetLeft()) / 1e6
    y = (bb.GetBottom() - pos.y) / 1e6
    rows.append((ref, f"{x:.3f}", f"{y:.3f}", "Top" if fp.GetLayer() == pcbnew.F_Cu else "Bottom", f"{fp.GetOrientationDegrees():.1f}"))

with open(os.path.join(OUT, "difftxlarge-bom.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["Comment", "Designator", "Footprint", "LCSC Part #"])
    for (key0, fpn), refs in sorted(groups.items(), key=lambda kv: kv[1][0]):
        lcsc = key0 if key0.startswith("C") and key0[1:].isdigit() else ""
        val = comments[(key0, fpn)]
        w.writerow([val, ",".join(sorted(refs)), fpn, lcsc])
with open(os.path.join(OUT, "difftxlarge-cpl.csv"), "w", newline="") as f:
    w = csv.writer(f)
    w.writerow(["Designator", "Mid X", "Mid Y", "Layer", "Rotation"])
    for r in sorted(rows):
        w.writerow(r)

print(open(os.path.join(OUT, "difftxlarge-bom.csv")).read())
print(open(os.path.join(OUT, "difftxlarge-cpl.csv")).read())
print("zip:", os.path.getsize(os.path.join(OUT, "difftxlarge-gerbers.zip")), "bytes")
