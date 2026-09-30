# Installing PixelPlus

This guide gets a PixelPlus controller onto your network and ready to use. You need:

* a Raspberry Pi **Zero 2 W, 3, 4 or 5** (with or without a PixelPlus board: difftx,
  difftxlarge, diffsmart),
* a **microSD card of 4 GB or more** (8–32 GB recommended, "A1"/"A2" cards boot faster),
* a computer with an SD-card slot or a USB card reader,
* your Wi-Fi name and password, or a network cable.

There are three ways to make the SD card. Pick one:

| | Best for |
|---|---|
| [A. PixelPlus Imager](#a-pixelplus-imager-recommended) | Everyone. Downloads, writes, checks and sets up Wi-Fi in one go. |
| [B. Raspberry Pi Imager](#b-raspberry-pi-imager) | If you already use Raspberry Pi Imager. |
| [C. Any SD-card writer + `pixelplus.txt`](#c-any-sd-card-writer--pixelplustxt) | balenaEtcher, `dd`, or preparing many cards at once. |

Then [start the Pi](#first-start) and [open PixelPlus](#first-run-wizard).
Want the show leader on a PC or NAS instead of a Pi? See [Docker](#running-the-leader-in-docker).

---

## A. PixelPlus Imager (recommended)

1. Download **PixelPlus Imager** for Windows, macOS or Linux from the
   [latest release](https://github.com/tlchandler/PixelPlus/releases/latest)
   (`.msi`/`.exe`, `.dmg`, `.AppImage`/`.deb`).
2. Start it and follow the four steps:
   1. **Image** – the newest PixelPlus is already selected. (You can also pick an
      `.img` / `.img.xz` file you downloaded yourself.)
   2. **SD card** – insert the card; it appears automatically. Only removable drives are
      listed – never your computer's own disks. *Everything on the card is erased.*
   3. **Settings**
      * **Wi-Fi**: network name, password (click *Show* to check it), and your country.
        Leave Wi-Fi empty if you use a network cable or want to set it up from your
        phone later.
      * **Name**: what the controller is called on your network, e.g. `pixelplus-garage`.
        You'll open it at `http://pixelplus-garage.local`. Give every controller its own name.
      * **Role**: *Show leader* for your main controller (exactly one per show),
        *Follower* for every extra controller.
      * **Time zone**: taken from your computer.
      * *Security & remote login (optional)*: a password for the PixelPlus web page, and
        SSH for advanced users.
   4. **Write** – confirm, enter your computer password when asked (writing an SD card
      needs administrator rights), and wait. The card is written, **read back and
      checked**, and your settings are saved on it.
3. When it says *Your SD card is ready*, take the card out and put it in the Pi.

> Windows asks "Do you want to allow this app to make changes?" when PixelPlus Imager
> starts – it needs that to write SD cards. macOS asks for your password when you click
> *Write*; Linux shows a password dialog (pkexec).

## B. Raspberry Pi Imager

PixelPlus can be installed with the official
[Raspberry Pi Imager](https://www.raspberrypi.com/software/) **version 2.0 or newer**:

1. Open Raspberry Pi Imager → **App Options** (gear / "Customisation" menu) →
   **Content Repository** → **Use custom URL** and enter

   ```
   https://github.com/tlchandler/PixelPlus/releases/latest/download/pixelplus-imager.json
   ```

   (or start it from a terminal: `rpi-imager --repo <that URL>`).
2. Choose your Raspberry Pi model, then **PixelPlus (Trixie, 64-bit)**, then your SD card.
3. When asked *"Would you like to apply OS customisation settings?"* choose **Edit
   settings** and fill in hostname, Wi-Fi (SSID, password, country), time zone and
   – if you like – a user name/password and SSH. PixelPlus applies these on first boot,
   exactly like Raspberry Pi OS does.
4. Write the card.

Good to know:

* Raspberry Pi Imager **1.x** cannot customise the *Trixie* image (its settings would be
  silently ignored). Update to Imager 2, or use the *Bookworm* PixelPlus image, or set
  Wi-Fi in `pixelplus.txt` (method C).
* The **role** (leader/follower) and the PixelPlus web password are not part of Raspberry
  Pi Imager: choose them in the browser on first start, or put them in `pixelplus.txt`.
* If you set something in **both** Raspberry Pi Imager and `pixelplus.txt`, the value in
  `pixelplus.txt` wins.

## C. Any SD-card writer + `pixelplus.txt`

1. Download `pixelplus-<version>-trixie-arm64.img.xz` from the
   [latest release](https://github.com/tlchandler/PixelPlus/releases/latest).
2. Write it with any tool, e.g. [balenaEtcher](https://etcher.balena.io/), or on Linux:
   `xzcat pixelplus-*.img.xz | sudo dd of=/dev/sdX bs=4M conv=fsync status=progress`.
3. Take the card out and put it back in. A drive called **bootfs** appears. Open the file
   **`pixelplus.txt`** on it with any text editor (Notepad, TextEdit, …).
4. Fill in what you need and save:

   ```ini
   wifi_ssid=MyHome
   wifi_password=my secret password
   wifi_country=US
   hostname=pixelplus-garage
   role=follower
   timezone=America/Chicago
   ```

5. Eject the card properly, then put it in the Pi.

### Everything `pixelplus.txt` can do

The file on the card has a friendly explanation above every line. In short:

| Setting | Example | Meaning |
|---|---|---|
| `wifi_ssid` / `wifi_password` | `MyHome` / `secret123` | Your Wi-Fi. Case sensitive. Empty password = open network. |
| `wifi_country` | `US`, `GB`, `DE`, `AU` | Country the Pi is used in (legal Wi-Fi channels). |
| `wifi_hidden` | `yes` / `no` | Your network does not broadcast its name. |
| `wifi2_ssid` / `wifi2_password` | | A second network to try (phone hotspot, show site). |
| `hostname` | `pixelplus-garage` | Name on the network → `http://pixelplus-garage.local`. |
| `role` | `leader` / `follower` | Empty = choose in the browser. |
| `timezone` | `America/New_York` | Used by the schedule and sunset times. |
| `board` | `auto`, `difftx`, `difftxlarge`, `diffsmart`, `bare-pi` | Normally `auto` (read from the board's memory chip). |
| `ip_address` / `ip_gateway` / `ip_dns` | `192.168.1.50/24` / `192.168.1.1` / `192.168.1.1` | Fixed IP address (most people leave this empty). `dhcp` = back to automatic. |
| `ip_interface` | `wifi` / `ethernet` | Which connection gets the fixed address. |
| `ui_password` | | Password for the PixelPlus web page (at least 6 characters). |
| `ssh` | `on` / `off` | SSH remote login (user `pi`). Empty = keep as is. |
| `ssh_password` / `ssh_key` | | Password (8+ characters) or public key for SSH. |
| `hotspot` | `on` / `off` | The setup hotspot (below). |
| `hotspot_password` | `pixelplus` | 8–63 characters, or `none` for an open hotspot. If you keep the default, it applies only to the first setup; once the Pi has been online it uses its own password from `PIXELPLUS-HOTSPOT.txt` (and never an open hotspot). |
| `hotspot_timeout` | `75` | Seconds to wait for Wi-Fi at power-on before opening the hotspot. |

Rules:

* **Empty means "leave as it is".** A new card starts with everything empty, so nothing
  is overwritten by accident.
* The file is read **every time the Pi starts** and also when it is saved while the Pi is
  running. Only settings that changed are applied.
* **Passwords are removed from the file once they are applied** (they're stored safely
  on the Pi). You'll see a note like `# [applied 2026-10-01 18:22] Saved on the device…`.
  Type a new password to change it; the Wi-Fi name stays so you can see what is set.
* If something is wrong, the Pi writes **`pixelplus-errors.txt`** next to
  `pixelplus.txt` telling you what to fix. Everything else is still applied.
* Changing the SSID without typing a password means the new network is **open**. To keep
  the saved password, don't change the SSID line.

---

## First start

Put the card in the Pi, connect your PixelPlus board if you have one, and power on.

* The first start takes **1–2 minutes**. If a PixelPlus board is detected the Pi
  **restarts once by itself** to switch on the pixel outputs – that's normal.
* Then open **`http://<name>.local`** in a browser on the same network, e.g.
  `http://pixelplus.local` (or the name you chose).

**`.local` doesn't open?** Some Android phones and older Windows PCs don't support
`.local` names. Look in your router's list of connected devices for the name and open
its IP address instead (e.g. `http://192.168.1.57`). A connected screen also shows the
address on the Pi's console.

### No Wi-Fi? Use the setup hotspot

If the Pi can't join a Wi-Fi network within about a minute (wrong password, network not
set, router out of range) **and no network cable is plugged in**, it opens its own Wi-Fi
network:

* **Network name:** `PixelPlus-XXXX` (the four characters are unique to each Pi)
* **Password:** `pixelplus` while you set it up for the first time. Once the Pi has been
  online, the hotspot uses **its own password** instead (so a neighbour can't use it): it is
  written to **`PIXELPLUS-HOTSPOT.txt`** on the SD card (open the card on any computer) and
  shown in PixelPlus under **Settings → Network**. Choose your own with
  `hotspot_password=` in `pixelplus.txt`.

1. On your phone, join **PixelPlus-XXXX**.
2. A *Sign in to network* page opens by itself (if not, open any web page, or go to
   `http://10.42.0.1`).
3. Tap your Wi-Fi network, type its password, check the country, tap **Connect**.
4. The page shows the address to use, e.g. `http://pixelplus.local`. Your phone leaves the
   hotspot; reconnect it to your normal Wi-Fi and open that address after about a minute.

If the password was wrong, the hotspot comes back within a minute and the page tells you
what happened – just try again. Other details:

* **Leave it alone and it fixes itself:** if a known network comes back (e.g. the router
  was still starting after a power cut), the Pi checks every 5 minutes while no phone is
  connected to the hotspot, and rejoins it.
* **If the Wi-Fi goes away later** for more than 10 minutes (and no cable is connected),
  the hotspot opens again so you can reach the Pi, with its own password (see above). The
  wait is deliberately long: someone jamming your Wi-Fi for a moment can't make it switch.
* **Use PixelPlus without Wi-Fi:** on the setup page tap *Use PixelPlus without Wi-Fi*,
  stay connected to the hotspot and open `http://10.42.0.1` – handy at a show site with
  no internet. PixelPlus itself never needs the internet.
* Turn the hotspot off with `hotspot=off` in `pixelplus.txt`.

Why a password on the hotspot? An open setup network would let anyone nearby point your
controller at *their* network. The default password is public, so it only keeps out
passers-by; set your own `hotspot_password` if that matters to you.

## First-run wizard

The first time you open PixelPlus it asks a few questions (anything you already set in
the Imager or `pixelplus.txt` is filled in):

1. **Leader or follower?** Your main controller is the *leader*; it holds the show and the
   music. Extra controllers are *followers*: they show "Waiting to be adopted…" and appear
   on the leader under **Controllers → New controllers found** – click **Adopt**.
2. **Board** – detected automatically from the board's memory chip. If it isn't (e.g. a
   blank chip), pick it; PixelPlus can write the chip for you. Changing the board
   restarts the controller once.
3. **Show name, location and time zone** – used for sunset-based schedules.
4. **Password** (optional) for the web page.

Then import your xLights layout and sequences (see the in-app guide).

## Running the leader in Docker

A PC or NAS can be the show leader (the Pis with boards are followers). Requirements:
Linux host (or NAS with Docker/Container Manager), **host networking** (followers are
found by broadcast and mDNS – this does *not* work in Docker Desktop on macOS/Windows).

```sh
curl -fsSLO https://raw.githubusercontent.com/tlchandler/PixelPlus/main/docker/docker-compose.yml
docker compose up -d                       # PixelPlus on port 80
docker compose --profile tts up -d         # + on-device DJ voices (Kokoro TTS)
```

* Open `http://<nas-address>` (port 80). If port 80 is taken (Synology DSM uses it), set
  `PIXELPLUS_HTTP_PORT=8080` in the compose file or an `.env` file.
* Set `TZ=` to your time zone.
* Show data lives in the `pixelplus-data` volume (`/var/lib/pixelplus`); back it up or use
  PixelPlus snapshots.
* For show music through the NAS/PC speakers, uncomment the `devices: /dev/snd` lines.
* The image runs as an unprivileged user (uid 1000). If you bind-mount a folder instead of
  the named volume, `chown -R 1000:1000` it first.

## Updating

* **Pi:** Settings → Updates in PixelPlus (installs the new `pixelplus` package; the SD
  card does not need to be re-written).
* **Docker:** `docker compose pull && docker compose up -d`.

## Troubleshooting

| Problem | What to do |
|---|---|
| Can't find `http://pixelplus.local` | Wait 2 minutes after power-on. Try the IP address from your router. Make sure your phone/PC is on the same network (not a guest network). |
| Two controllers with the same name | The second one becomes `pixelplus-2.local`. Give each its own `hostname=`. |
| Hotspot doesn't appear | It only opens when there is no working network **and** no cable. Unplug the cable, wait ~75 s. Pi 3/Zero 2 W: 2.4 GHz only. |
| Wi-Fi network not listed on the setup page | 5 GHz networks are invisible to Pi Zero 2 W / Pi 3 (older ones). Use *Other network* for hidden networks. |
| Settings in `pixelplus.txt` ignored | Look for `pixelplus-errors.txt` on the card, and `/var/log/pixelplus-firstboot.log` on the Pi. |
| Pixels don't light | Check the board was detected (Controllers page). Run *Test* on a prop. See the board's README. |
| End of a long string stays dark | The pixel output's string length is set when the controller starts. The Dashboard / Controllers page shows **Apply & reboot** when a string got longer than that. |

Advanced: log in with SSH (`ssh pi@pixelplus.local`, after setting `ssh=on` and
`ssh_password=`, or turning SSH on in **Settings → Security**), then `pixelplus doctor`, `journalctl -u pixelplusd -f`,
`journalctl -u pixelplus-netwatch`.
