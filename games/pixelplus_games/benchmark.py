"""Measure whether this player can run the game in real time.

    python3 -m pixelplus_games.benchmark [rom.nes] [WIDTHxHEIGHT]

Runs the emulator flat out (no audio, nothing sent to pixelplusd) and reports how
much of each 1/60 s frame the work takes.  Run it while your show is playing
to see what is left over on a busy player.
"""

import os
import sys
import time

import numpy as np

from . import config
from .display import Scaler
from .libretro import Core, find_core

FRAMES = 600


def main(argv):
    rom = argv[0] if argv else config.smb_rom_path()
    if not os.path.isfile(rom):
        print("No ROM at %s; pass the path of a .nes file" % rom)
        return 2
    size = argv[1] if len(argv) > 1 else "80x40"
    try:
        width, height = (int(v) for v in size.lower().split("x"))
    except ValueError:
        print("Matrix size must look like 80x40")
        return 2
    core_path = find_core(config.core_path())
    if not core_path:
        print("No libretro NES core found (apt-get install libretro-nestopia)")
        return 2
    system_dir = config.games_dir() if os.path.isdir(config.games_dir()) else os.getcwd()
    core = Core(core_path, system_dir, config.core_options())
    core.load_game(rom)
    scaler = Scaler(width, height)
    core.buttons = 1 << 7  # hold right so something happens

    core.capture_video = False
    t = time.perf_counter()
    for _ in range(FRAMES):
        core.run()
    emu = (time.perf_counter() - t) / FRAMES

    core.capture_video = True
    core.run()
    t = time.perf_counter()
    for _ in range(FRAMES // 4):
        core.run()
        img = scaler.scale_frame(core.frame)
        _ = np.ascontiguousarray(img).tobytes()
    emu_plus_out = (time.perf_counter() - t) / (FRAMES // 4)
    out = max(0.0, emu_plus_out - emu)

    print("Core: %s   Matrix: %dx%d" % (core.name, width, height))
    print("Emulating one frame:          %6.2f ms" % (emu * 1000))
    print("Scaling + output per frame:   %6.2f ms" % (out * 1000))
    for fps in (40, 20):
        load = (emu * 60 + out * fps) / 1.0
        print("Matrix at %d fps: %3.0f%% of one CPU core%s" % (
            fps, load * 100, "" if load < 0.6 else "  <-- too close to the limit"))
    print("Guide: under ~60% leaves room for pixelplusd, audio and the web server.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
