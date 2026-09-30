# Christmas PCBs

Each design has its own folder. The top of each folder is the current working copy (open the
`.kicad_pro` there); earlier revisions are frozen under `archive/`.

## Tools

All designs are developed in **KiCad 10.0**, installed at `C:\Program Files\KiCad\10.0\`:

| Binary | Path | Used for |
|---|---|---|
| KiCad | `C:\Program Files\KiCad\10.0\bin\kicad.exe` | opening the projects |
| kicad-cli | `C:\Program Files\KiCad\10.0\bin\kicad-cli.exe` | ERC/DRC, gerber and render export |
| KiCad Python | `C:\Program Files\KiCad\10.0\bin\python.exe` | running the `gen_*.py`, `export_jlc.py`, `verify.py` etc. scripts |

The scripts need KiCad's bundled Python (it provides `pcbnew`), so run them from the design's folder, e.g.

```
"C:\Program Files\KiCad\10.0\bin\python.exe" gen_pcb.py
```

## Designs

| Folder | Board | Current | Archived revisions |
|---|---|---|---|
| `difftx/` | FPP Remote pHAT — RS-422 transmitter on the Pi Zero 2 W outline | rev E (65 x 30 mm). Rev D, the one built, has port 3 reversed: see `CLAUDE.md` | `archive/revA`, `archive/revB`, `archive/revC_66mm`, `archive/revD` |
| `diffrx/` | Chandler 4D/8P Differential Receiver | work after rev C (board and `grid_router.py` changed since the release) | `archive/revB`, `archive/revC_release` |
| `diffsmart/` | Chandler 4D/8P Smart Receiver — diffrx rev C + Pi header, RX/TX mode switch | v1.00 | — |
| `difftxlarge/` | 60-output FPP transmitter — 15 × RJ45 (Falcon differential), 3 latch banks, RTC, power monitor, fan thermostat, line out | rev A (310 × 206 mm, not yet ordered) | — |

Each design's `VERIFICATION.md` has its check record; `jlcpcb/` holds the gerbers, BOM and CPL for ordering.

`repoint_3d_models.py` fixes 3D model paths that point at folders that no longer exist (the parts were first
downloaded under `Documents/jumperless/...`), repointing them at each design's own `jlc/jlc.3dshapes`. Run it
with `--apply`; without it, it only reports.

`_duplicates/` holds files that were byte-identical copies of `difftx/archive/revB` files left at the
top level (`fab/`, `difftx_pcb.pdf`). Safe to delete.
