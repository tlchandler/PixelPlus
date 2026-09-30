"""
Repoint 3D model paths that no longer exist (the parts were first downloaded under Documents/jumperless/...,
which has since gone) to the copy of the same file inside each design, relative to the KiCad project
(${KIPRJMOD}), so the designs render wherever the folder lives.

  python repoint_3d_models.py            # report only
  python repoint_3d_models.py --apply    # rewrite the files

Only model paths that do not exist on disk are touched; KiCad's own ${KICAD10_3DMODEL_DIR} models and paths
that already resolve are left alone. A board's project directory is the board's folder; a library footprint's
is the design folder that holds its jlc/ or <name>.pretty/ library. Candidates, in order: the project's own
jlc/jlc.3dshapes, then the parent design's (for archived revisions without their own copy).
"""
import os, re, sys

ROOT = os.path.dirname(os.path.abspath(__file__))
APPLY = "--apply" in sys.argv
MODEL = re.compile(r'\(model "([^"]+)"')


def project_dir(path):
    d = os.path.dirname(path)
    if path.endswith(".kicad_mod"):              # .../<design>/jlc/jlc.pretty/x.kicad_mod or .../<design>/<name>.pretty/x
        d = os.path.dirname(d)
        if os.path.basename(d) == "jlc":
            d = os.path.dirname(d)
    return d


def resolve(proj, name):
    for rel in ("jlc/jlc.3dshapes", "../../jlc/jlc.3dshapes", "../jlc/jlc.3dshapes"):
        if os.path.exists(os.path.normpath(os.path.join(proj, rel, name))):
            return "${KIPRJMOD}/" + rel + "/" + name
    return None


def exists(p, proj):
    if p.startswith("${KICAD"):
        return True                              # KiCad's own library
    return os.path.exists(os.path.normpath(p.replace("${KIPRJMOD}", proj)))


changed_files, fixed, unresolved = 0, 0, []
for dirpath, dirs, files in os.walk(ROOT):
    if "difftxlarge" in dirpath.replace("\\", "/").split("/"):
        continue                                 # already done
    for fn in files:
        if not fn.endswith((".kicad_pcb", ".kicad_mod")):
            continue
        path = os.path.join(dirpath, fn)
        proj = project_dir(path)
        text = open(path, encoding="utf-8").read()
        n_here = [0]

        def fix(m):
            p = m.group(1)
            if exists(p, proj):
                return m.group(0)
            name = os.path.basename(p.replace("\\", "/"))
            new = resolve(proj, name)
            if new is None and p.startswith("${KIPRJMOD}/"):
                # an archived revision still pointing at the design folder's own library, two levels up
                rel = p[len("${KIPRJMOD}/"):]
                if os.path.exists(os.path.normpath(os.path.join(proj, "../..", rel))):
                    new = "${KIPRJMOD}/../../" + rel
            if new is None:
                unresolved.append((os.path.relpath(path, ROOT), p))
                return m.group(0)
            n_here[0] += 1
            return f'(model "{new}"'

        new_text = MODEL.sub(fix, text)
        if n_here[0]:
            changed_files += 1
            fixed += n_here[0]
            print(f"{n_here[0]:4d}  {os.path.relpath(path, ROOT)}")
            if APPLY:
                open(path, "w", encoding="utf-8", newline="\n").write(new_text)
print(f"\n{'applied' if APPLY else 'would fix'}: {fixed} model paths in {changed_files} files")
print(f"unresolved: {len(unresolved)}")
for f, p in unresolved[:40]:
    print("   ", f, "->", p)
