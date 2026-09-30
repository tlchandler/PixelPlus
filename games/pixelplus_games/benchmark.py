"""Measure whether this player can run the game in real time.

    cd /home/fpp/media/plugins/fpp-mariobros && python3 -m mario.benchmark [rom.nes]

Runs the emulator flat out (no audio, nothing sent to FPP) and reports how
much of each 1/60 s frame the work takes.  Run it while your show is playing
to see what is left over on a busy player.
"""

import os
import sys
import time

import numpy as np

from . import config
from .display import Scaler, frame_to_rgb
from .libretro import Core, find_core

FRAMES = 600


def main(argv):
    rom = argv[0] if argv else config.ROM_PATH
    if not os.path.isfile(rom):
        print("No ROM at %s; pass the path of a .nes file" % rom)
        return 2
    core_path = find_core(config.load().get("CorePath", ""))
    if not core_path:
        print("No libretro NES core found (apt-get install libretro-nestopia)")
        return 2
    core = Core(core_path, config.DATA_DIR if os.path.isdir(config.DATA_DIR) else os.getcwd())
    core.load_game(rom)
    scaler = Scaler(80, 40)
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
        img = scaler.scale(frame_to_rgb(core.frame))
        _ = np.ascontiguousarray(img).tobytes()
    emu_plus_out = (time.perf_counter() - t) / (FRAMES // 4)
    out = max(0.0, emu_plus_out - emu)

    print("Core: %s" % core.name)
    print("Emulating one frame:          %6.2f ms" % (emu * 1000))
    print("Scaling + output per frame:   %6.2f ms" % (out * 1000))
    for fps in (40, 20):
        load = (emu * 60 + out * fps) / 1.0
        print("Matrix at %d fps: %3.0f%% of one CPU core%s" % (
            fps, load * 100, "" if load < 0.6 else "  <-- too close to the limit"))
    print("Guide: under ~60% leaves room for fppd, audio and the web server.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
