# difftxlarge — research notes (2026-09-25)

Goal: a large-format sibling of `../difftx` (FPP Remote pHAT) with as many RS-422 RJ45 outputs as practical,
landing on `../diffrx` / `../diffsmart`, with zip-tie strain relief at each jack and audio output.
Nothing below is designed yet; these are the facts the design decisions rest on. [V] = verified against a
source; [I] = inference.

## How many outputs the Pi can drive (FPP DPIPixels)

Source: FPP master (commit 98bf09e, ~10.1.x) `src/non-gpl/DPIPixels/DPIPixels.cpp`,
`capes/drivers/pi/vc4-kms-dpi-fpp.dts`, `src/boot/FPPINIT_Config.cpp`, `www/help/channeloutputs.php`;
FPP 10.0 release notes (21 Aug 2026).

- **Direct mode: 24 outputs max** [V] — `GetDPIPinBitPosition()` accepts GPIO4-27 = DPI_D0..D23 only.
  24 / 4 per RJ45 = **6 RJ45**.
- **Latch mode: up to 80 outputs** [V, code comment] — `MAX_DPI_PIXEL_LATCHES = 4`; 20 data pins x 4 latch
  banks, full string length per output since FPP 10. Up to 3 banks keeps T0H/T1H at 312/729 ns; 4 banks
  gives 417/833 ns. Worked example in `docs/samples/eeproms/sample-pi-strings-52.json`. The Kulp K8-Pi
  uses latches. 3 banks x 20 = 60 outputs = **15 RJ45**; 4 banks x 20 = 80 = **20 RJ45**.
- GPIO0-3 are NOT muxed by the FPP overlay (`dpi_no_gpio`), so i2c-1 (GPIO2/3) stays free for the cape
  EEPROM and any other I2C devices [V]. All other header GPIO (UART, SPI, I2S, PWM) is consumed [I].
- Per-string limit: 4800 channels = **1600 RGB pixels**; about fps x channels <= 96,000, so 1600 px at
  about 20 fps, 800 px at 40 fps [V]. Fixed 38.4 MHz clock, so it's the same on every Pi.
- Pi 3B+/4/5 all supported in current FPP; Pi 5 muxes alt1 instead of alt2 [V]. There are known Pi 5
  pin-float issues (#2895, #2768). **Put pull-downs on every driver input.**
- **Licensing** [V in code]: with an unsigned/unlicensed cape EEPROM, outputs 3+ are capped at **50 pixels**.
  A 24/60/80-output board needs a signed EEPROM or FPP license keys. Community reports put keys at about
  US$10 per 8 outputs [unverified].
  This also applies to the existing 4-port difftx (ports 3-4).

## Audio

- DPIPixels coexists with onboard audio (`snd_bcm2835` is not blacklisted) [V]. Pi 3/4 analog audio uses
  internal GPIO40/41, so there's no DPI conflict [I].
- Pi 5 has no 3.5 mm jack. Use a USB sound card or HDMI [V]. I2S DAC on the header is impossible: GPIO18-21
  are pixel outputs.
- Onboard option: TPA3116D2 (C50144) class-D amp on 12 V, about 2 x 8-9 W into 8 ohm at 12 V [I], fed by a
  3.5 mm patch cable from the Pi (or USB dongle on Pi 5). Avoid PAM8403: it's 5 V-only.

## Power

- Pi 4 needs 5 V 3 A, Pi 5 needs 5 V 5 A [V]. The current difftx's K7805-2000R3 (2 A) is **too small**.
- Pi 5 fed via header pins: set `usb_max_current_enable=1` (config.txt) or `PSU_MAX_CURRENT=5000`
  (bootloader EEPROM), or USB is limited to 600 mA [V].
- Buck: **TPS56637RPAR** (C841386), 4.5-28 V in, 6 A, PG pin. Datasheet 5 V values: L 3.3 uH
  (PSPMAA0805-3R3M, C2962881), 73.2 k / 10 k divider, 3-4 x 22 uF out. Budget alternative: SY8205FCC
  (C111875), 5 A with no margin.
- HAT guide: a back-powering HAT should protect against simultaneous USB power (ideal diode) [V].

## Board size and cost (JLCPCB, checked 2026-09-25)

- **"Large Size" threshold: area > 650 cm²** (e.g. >260 x 250 mm) for both PCB and PCBA [V].
  PCB: small per-50 cm² fee. PCBA: flat **$57.46** [V]. Stay at or below 650 cm².
- The real price step is leaving 100 x 100 mm on 4-layer (the $25 engineering fee). Qty 5, 4-layer:
  200x100 $37, 200x150 $44, 250x150 $48 [V, live quotes]. 2-layer is roughly a third of that.
- PCBA single-board limit: 470 x 500 mm economic [V]. Through-hole joints are $0.0164 each, so RJ45 cost is
  negligible.

## Parts

- RJ45: keep **C385834** (Ckmtw R-RJ45R08P-A004, same as difftx/diffrx); big stock. No 1x6 ganged part
  in stock. 1x4 ganged unshielded: C2828087.
- Per-port activity LEDs without GPIO: 74HCT245 (C5979, **Basic**) buffering the driver inputs. Never put
  LEDs directly on 24 GPIOs (about 50 mA total GPIO budget).
- I2C options on i2c-1: RTC DS3231MZ+ (C107410) or PCF8563T (C7440, Preferred) + CR2032 holder (C70377);
  INA226 (C49851, 36 V bus) on 12 V in; TMP102 (C5692997); PCA9555 (C42420607); SSD1306 OLED header (0x3C).

## Strain relief

- diffrx rev C already uses 3.0 mm NPTH round holes (`ZipTie_3.0mm`) for 2.5 mm ties.
- For a cable (not just the jack) to be tied down, the cable has to lie over board. Either set the jacks
  back from the edge or add a tongue, and put a slot pair about 25-35 mm in front of the jack face,
  behind the plug boot, never over the latch [I].
  Suggested slot: 2.0 x 4.5 mm NPTH oval, which takes 2.5 and 3.6 mm ties. JLC min NPTH slot is 1.0 mm [V].
