# difftxlarge — design decisions (working notes, 2026-09-25)

Chosen by the user: **15 RJ45 = 60 outputs, 3 latch banks**, plus features 2-11, 13 and 14 from the
proposal (see RESEARCH.md for the sources).

## Architecture

```
Pi (any 40-pin: 3B+/4/5) --40-way ribbon--> J_PI box header
   GPIO4..23  = D0..D19   (20 data lines)
   GPIO27/26/25 = LE0/LE1/LE2 (P1-13 / P1-37 / P1-22, the latch pins FPP's own 52-output sample uses)
   GPIO2/3 = i2c-1: cape EEPROM 0x50, DS3231 0x68, INA226 0x40, LM75B 0x48/0x49, OLED header 0x3C
      |
   pull-downs (Pi side, a row between J16 and the buffers) -> 3 x 74AHCT541 buffers (5 V, TTL inputs)
   -> 33 R series -> 23-line bus
      |
   9 x 74AHCT573 latches: bank b = LE_b, latch m of bank b takes D(8m..8m+7)
      |
   output k (0..59): bank b = k // 20, bit i = k % 20 -> jack J = k // 4, port = k % 4
   15 x (AM26C31 + 4 x PSM712 + 4 activity LEDs) -> RJ45 (Falcon pinout, +/-: 1/2, 3/6, 5/4, 7/8)
```

- The latch pulse FPP generates is **52 ns HIGH** (two framebuffer pixels), data set up 78 ns before LE falls
  and held 26 ns after (DPIPixels.h WriteLatchedDataAtPosition). That's why AHCT: the 74HCT573's 9-15 ns hold time would be marginal against the 26 ns hold.
- The pull-downs keep every line low while the Pi boots (GPIO0-8 come up with pull-ups) and when it's
  absent. FPP also parks the lines low at shutdown. Latch outputs are undefined only until the first LE
  pulse, which leaves static levels on the cable; a static level carries no pixel data.
- **Licensing:** latches work unlicensed (DPIPixels treats an unlicensed cape as 2 licensed outputs before its
  latch check), but outputs 3-60 are capped at 50 pixels until the cape EEPROM is signed for >= 60 outputs.

## Power

- 12 V in -> ATO blade fuse -> high-side P-FET reverse protection -> TVS -> INA226 shunt -> +12V.
- **5V_PI**: TPS56637 6 A buck -> **USB-C power-out receptacle** -> short C-to-C cable -> the Pi's own
  USB-C input.
  - This is not through the ribbon. On a Pi, 5 V reaches the header only on pins 2 and 4, so it would be
    two 28 AWG ribbon wires and two IDC contacts at about 1 A each. That's fine for diffsmart's 2 A, but
    not for a Pi 4 (3 A) or a Pi 5 (5 A).
  - Keeping 5 V off the header also means the board's 5 V never meets another supply. That removes the
    back-feed problem feature 9 was going to solve with an ideal diode.
  - Rp on CC advertises 3.0 A. A Pi 5 then needs `usb_max_current_enable=1` for full USB current.
- **5V_DRV**: second TPS56637 (same part, so no extra JLC feeder fee) for the buffers, latches, drivers and
  LEDs. 60 terminated pairs at about 25 mA each is about 1.5 A (diffrx terminates 120 Ω).
- I2C devices run from the Pi's 3V3 (ribbon pins 1/17), exactly as on diffsmart.

## Floorplan (y down, mm) — board 310 x 206 = 639 cm², under JLC's 650 cm² "Large Size" line (bands below 64 are 6 mm lower as built)

| y | band |
|---|---|
| 0-64 | Pi (85 x 56, rotated 180°: ports toward the top/left edges, header toward the board) at the top-left; power, I2C, audio, fan, OLED on the right |
| 64-87 | J16 box header under the Pi's header, pull-downs, buffers, series resistors |
| 89-104 | 23-line bus (B.Cu horizontals at 0.7 mm pitch) |
| 100-114 | 9 latches, each above the jack pair it feeds |
| 118-146 | per-jack cell: activity LEDs, AM26C31, PSM712s |
| 149-165 | RJ45s, 20 mm pitch, x = 15 ... 295, openings facing the bottom edge |
| 165-200 | cable band: plug + boot lie on the board, zip-tie slot pair per jack about 27 mm in front of the jack face, port labels / write-on boxes |

Bank b occupies x = 5+100b ... 105+100b. It's routed once and copied twice. Only the LE line differs
between banks.

The board as built is recorded in VERIFICATION.md; where this file and that one differ, VERIFICATION.md is current.
