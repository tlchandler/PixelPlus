# PixelPlus ESP32 sensor node

A small Wi-Fi board in the yard that tells your PixelPlus show about motion,
button presses, broken light beams and opened doors — and, optionally, how
much current a receiver's pixel supply draws. PixelPlus turns those into
**surprises**: a short sparkle on the candy canes when someone walks up the
driveway, "Jingle Bells" on the porch when the doorbell button is pressed,
without stopping the song that is playing.

- Boards: **ESP32-C3 DevKitM-1** (recommended), ESP32-S3 DevKitC-1, classic ESP32 DevKit.
- Inputs: up to 8 — PIR motion (HC-SR501, AM312), push buttons, IR break-beam
  pairs, reed/door contacts, and INA219/INA226 current sensors on I²C.
- Protocol: authenticated UDP on port **32422** to the show leader
  (`docs/ARCHITECTURE.md` §7.5, §12.16).

## Build and flash

You need [PlatformIO](https://platformio.org) (`pip install platformio`, or the VS Code extension).

```sh
cd firmware/esp32-sensor
pio run -e esp32c3 -t upload        # ESP32-C3 DevKitM-1 over USB
pio run -e esp32s3 -t upload        # ESP32-S3 DevKitC-1
pio run -e esp32dev -t upload       # classic ESP32 DevKit
pio device monitor                  # log at 115200 baud
pio test -e native                  # protocol and debounce tests on your PC
```

`esp32dev-classic` builds with the stock `espressif32` platform (Arduino-ESP32
2.x) if you can't use the pioarduino platform (Arduino-ESP32 3.x) the other
environments use.

## First start: Wi-Fi setup

1. Power the board. With no Wi-Fi saved it opens a hotspot named
   **PixelPlus-Sensor-XXXX** (the LED blinks **blue**).
2. Join it with your phone. The setup page opens by itself (or browse to
   `http://192.168.4.1`).
3. Pick the Wi-Fi your PixelPlus show uses, enter its password and a name
   ("Driveway", "Porch"), and tap **Save and connect**. The board restarts
   and joins your network (LED: a short **amber** blink every 2 s = waiting to
   be added).
4. In PixelPlus open **Settings → Sensors**. The board shows up under
   *New sensors found*; tap **Add**. The LED goes dark (adopted).
5. Name the inputs, set their pins, then create a trigger in
   **Settings → Triggers**: *When* "Driveway · Motion 1", *do* a surprise.

If the saved Wi-Fi disappears for 2 minutes the hotspot comes back (the
board keeps trying the saved network every minute and leaves the hotspot
once it works again).

### BOOT button

| Press | Effect |
|---|---|
| short (< 2 s) | allow a new show leader to adopt it for 10 minutes (LED blinks **purple**); also allows changing Wi-Fi from its status page |
| hold 10 s | factory reset: forgets Wi-Fi, show leader and settings |

### LED codes

| LED | Meaning |
|---|---|
| slow blue | setup hotspot is open |
| amber blink every 2 s | on Wi-Fi, not added to a show yet |
| fast white | "Identify" pressed in PixelPlus |
| purple | adoption window open (after a short BOOT press) |
| off | working normally (a short white flash on each trigger) |

## Wiring

All inputs are 3.3 V logic. **Never feed 5 V or 12 V into a GPIO.** Default
pins (a fresh node announces these; change them in Settings → Sensors):

| Board | Motion `pir1` | Button `btn1` | Beam | I²C SDA / SCL | BOOT | LED |
|---|---|---|---|---|---|---|
| ESP32-C3 DevKitM-1 | GPIO 4 | GPIO 5 | GPIO 7 | GPIO 1 / GPIO 3 | GPIO 9 | GPIO 8 (RGB) |
| ESP32-S3 DevKitC-1 | GPIO 4 | GPIO 5 | GPIO 6 | GPIO 8 / GPIO 9 | GPIO 0 | GPIO 48 (RGB) |
| ESP32 DevKit | GPIO 27 | GPIO 26 | GPIO 25 | GPIO 21 / GPIO 22 | GPIO 0 | GPIO 2 |

Avoid strapping pins (C3: 2, 8, 9; ESP32: 0, 2, 5, 12, 15) for inputs.

### PIR motion sensor (HC-SR501 or AM312)

```
 HC-SR501 (needs 5 V)            ESP32-C3 DevKitM-1
 ┌──────────────┐
 │ VCC ─────────┼──────────────── 5V
 │ OUT ─────────┼──────────────── GPIO 4      (3.3 V output: safe)
 │ GND ─────────┼──────────────── GND
 └──────────────┘
 AM312 mini PIR: VCC → 3V3 instead of 5V.
```

Kind **motion**, *active low* off. Turn the HC-SR501's "time" pot fully
counter-clockwise (shortest) and use PixelPlus' **hold** setting instead; set
its jumper to "repeat" (H). Outdoors, shade it from the street: car headlights
and warm exhaust trigger PIRs — test at night.

### Push button / doorbell

```
          ┌──── GPIO 5  (internal pull-up)
 button  ─┤
          └──── GND
```

Kind **button**, *active low* **on**. For long cable runs (> 5 m) add a
1 kΩ series resistor and a 100 nF capacitor from the GPIO to GND.

### IR break-beam pair (3 mm/5 mm, e.g. Adafruit 2167)

```
 Emitter: red → 3V3 (or 5V), black → GND
 Receiver: red → 3V3, black → GND, white (open collector) → GPIO 7
```

Kind **beam**, *active low* **off** (the receiver pulls low while the beam is
intact; the internal pull-up keeps it high when broken → PixelPlus sees
"active" when someone walks through). If your receiver works the other way,
toggle *active low*.

### Door / reed contact

```
 reed switch: one wire → GPIO, other → GND     (kind "contact", active low on)
```

### Current sensor on a receiver's pixel supply (INA226 / INA219)

Measures the real current of a receiver's 12 V pixel bus (the PixelPlus TX
board's own INA226 cannot see pixel current). Use an INA226 module with an
**external shunt** sized for the bus — the on-board 0.1 Ω shunt is only good
to ~0.8 A.

```
 12 V PSU (+) ──[ external shunt 75 mV/50 A = 1.5 mΩ ]── receiver 12 V (+)
                    │                        │
                 IN+ ┘                        └ IN-      INA226 module
 12 V PSU (−) ─────────────────────────────── GND ────── GND (shared with ESP32)
                                               VBUS ───── receiver 12 V (+)   (bus voltage)
 INA226 VCC → 3V3,  SDA → GPIO 1,  SCL → GPIO 3   (ESP32-C3)
 Address pins A0/A1 to GND → 0x40 (up to 16 modules: 0x40–0x4F)
```

In Settings → Sensors add an input of kind **current**: *pin* is the I²C
address (64 = 0x40) and *shunt* its resistance in milliohms (1.5 for
75 mV/50 A; 100 for a bare module). Remove the module's own R100 shunt when
you wire an external one. The node reports amps and bus volts every 10 s;
link the input to a power supply in Settings → Power so the limiter can use
the measured value.

> Safety: the shunt carries the full pixel current. Use wire and a fuse rated
> for it, keep the shunt on the **positive** side as drawn, and mount it
> inside the receiver's enclosure.

## How it talks to PixelPlus

- **Discovery**: a broadcast `sbeacon` every 2 s (10 s once adopted).
- **Adoption** (trust on first use): the leader POSTs
  `http://<node>/adopt {leaderId, leaderUrl, sensorPort, dh}`; the node
  answers `{id, dh, proof, hw, ver, inputs}`. Both sides compute
  `key = HMAC-SHA256(X25519 shared secret, "pixelplus-sensor-key-v1\n…")`;
  the key is stored in NVS and never sent. A node adopted by a show refuses
  other leaders until it is released, reset (BOOT 10 s), or a short BOOT
  press opens a 10-minute window.
- **Events / heartbeats**: `sevent` on every input change (retried at 100,
  200 and 400 ms until the leader's `sack`), `sstatus` every 10 s. Each
  datagram carries the node's boot id, a sequence number and an HMAC; the
  leader drops replays and events addressed to an older leader boot.
- **Configuration**: fetched from
  `GET <leader>/api/v1/cluster/sensor-config/<id>`, signed with the node key
  (`X-PixelPlus-Auth`) and answered with `X-PixelPlus-Reply`. The node
  refetches when a `sack` names a different config version.
- **Release**: the leader POSTs `/release` signed with the key.

Test vectors shared with the Rust leader: [`test/vectors.json`](test/vectors.json)
(generated independently in Python; checked by `pio test -e native` here and
by `cargo test` in `crates/pixelplus-daemon`).

## Troubleshooting

| Symptom | Try |
|---|---|
| Not listed in *New sensors found* | Same Wi-Fi/VLAN as the leader? UDP broadcast to port 32422 must reach it. The node's status page (its IP in your router) says whether it is on Wi-Fi. |
| "belongs to another show" | Release it on the other show, or short-press BOOT, then Add again. |
| Motion fires by itself | Shade the PIR, lower its sensitivity pot, raise *hold*, add a trigger *active window* and cooldown. |
| Events arrive late | Weak Wi-Fi (Sensors page shows dBm; below −75 dBm is poor). Move the node or add an access point. |
| Current reads 0 | Wrong I²C address or SDA/SCL swapped; the Sensors page shows no current until the module answers. |
