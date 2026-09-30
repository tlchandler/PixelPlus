# Christmas PCBs: notes for Claude

See `README.md` for the repo layout, KiCad 10 paths and how to run the generator scripts.

## IMPORTANT: RJ45 port 3 polarity (Falcon differential pinout)

The Falcon differential pinout puts port 3 on the blue pair as **pin 5 = + (white/blue), pin 4 = − (solid blue)**.
Ports 1, 2 and 4 follow pin order: 1(+)/2(−), 3(+)/6(−), 7(+)/8(−).
Dan Kulp on the Falcon forum: "blue (5, 4) is for port 3"
(https://falconchristmas.com/forum/index.php?topic=15638.0).

**difftx rev D (built and in use) has port 3 reversed: 4(+)/5(−).** Confirmed 2026-09-29 (a cable with pins 4/5 swapped fixed it) on a PixelController SRx1 v5.01
receiver. Ports 1, 2 and 4 worked, but port 3 showed white sparkle and ignored data. The receiver's own test mode was fine.
- This can't be fixed in software. The Pi 3B+/Zero DPI hardware drives the data pins low during blanking, so
  inverting one port's bits in DPIPixels corrupts the line gaps and the reset.
- Workaround for rev D boards: a short T568B patch lead with **pins 4 and 5 (blue and white/blue) swapped at
  one end**. It's the same twisted pair, so the twisting still works.
- Don't copy difftx rev D's J1 pin 4/5 wiring into any new design.

**Fixed 2026-09-29 in diffrx, diffsmart and difftxlarge rev A**, which had copied the same 4(+)/5(−). They now use 5(+)/4(−).
Each one's `VERIFICATION.md` has a "Port 3 polarity fix" section. Backups from before the fix are in `_backups/`.
- Those boards work with Falcon gear on a straight lead. **difftx rev D paired with diffrx or diffsmart needs the 4/5-swapped
  lead on port 3**, just like rev D with a Falcon receiver.
- **difftx rev E (2026-09-29, the current working copy) fixes it:** J1 pin 5 = 2Y, pin 4 = 2Z, cape EEPROM version 1.1. Rev D is
  in `difftx/archive/revD`. A rev D board's EEPROM (version 1.0) identifies it in FPP's Cape page.
