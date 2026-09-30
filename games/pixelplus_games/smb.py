"""Super Mario Bros. specifics: RAM addresses, level select, score.

Addresses and behaviour come from doppelganger's SMBDIS.ASM disassembly.
Level select works the way the game's own "continue" does: while the title
menu is up, WorldNumber/LevelNumber/AreaNumber are written and Start is
pressed; StartWorld1 then calls LoadAreaPointer with those values.
"""

import random

from .libretro import JOYPAD_START

OPER_MODE = 0x0770            # 0 title, 1 game, 2 victory, 3 game over
OPER_MODE_TASK = 0x0772
DEMO_TIMER = 0x07A2
GAME_ENGINE_SUBROUTINE = 0x000E  # 8 = normal player control
NUMBER_OF_LIVES = 0x075A
LEVEL_NUMBER = 0x075C
WORLD_NUMBER = 0x075F
AREA_NUMBER = 0x0760
PLAYER_SCORE_DISPLAY = 0x07DD  # six decimal digits, one per byte: millions down to tens (the ones digit is always 0)

MODE_TITLE, MODE_GAME = 0, 1
TITLE_MENU_TASK = 3

ALL_LEVELS = ["%d-%d" % (w, l) for w in range(1, 9) for l in range(1, 5)]


def parse_levels(spec):
    """'all' or '1-1, 1-2, 4-1' -> list of 'W-L' strings (invalid entries dropped)."""
    spec = (spec or "all").strip().lower()
    if spec in ("", "all"):
        return list(ALL_LEVELS)
    levels = []
    for part in spec.replace(";", ",").split(","):
        part = part.strip()
        if part in ALL_LEVELS and part not in levels:
            levels.append(part)
    return levels or list(ALL_LEVELS)


def area_number(world, level):
    """AreaNumber (0-based) for a 1-based world and level.

    In worlds 1, 2, 4 and 7 the level-2 intro (walking into the pipe) is an
    area of its own, so levels 2-4 sit one area later.
    """
    area = level - 1
    if world in (1, 2, 4, 7) and level >= 2:
        area += 1
    return area


def random_level(levels, rng=random):
    return rng.choice(levels)


class SMB:
    LIVES = 9

    def __init__(self, core):
        if core.ram is None or len(core.ram) < 0x800:
            raise RuntimeError("The emulator core does not expose NES RAM; level select needs it")
        self.core = core
        self.ram = core.ram

    def _write_level(self, world, level):
        self.ram[WORLD_NUMBER] = world - 1
        self.ram[LEVEL_NUMBER] = level - 1
        self.ram[AREA_NUMBER] = area_number(world, level)
        self.ram[NUMBER_OF_LIVES] = self.LIVES

    def boot_to_level(self, name, max_frames=3000):
        """Reset the console and fast-forward (no output) into level ``name``.

        Returns True once Mario is under player control in that level.
        """
        world, level = (int(x) for x in name.split("-"))
        core, ram = self.core, self.ram
        core.buttons = 0
        core.reset()
        frames = 0

        # 1. Wait for the title menu, with the demo timer still running
        #    (pressing Start after it expires just resets the title screen).
        while frames < max_frames:
            core.run(); frames += 1
            if ram[OPER_MODE] == MODE_TITLE and ram[OPER_MODE_TASK] == TITLE_MENU_TASK and ram[DEMO_TIMER]:
                break

        # 2. Write the level and tap Start (Start is edge-triggered, so
        #    alternate press/release) until the game mode begins.
        pressed = False
        while frames < max_frames and ram[OPER_MODE] == MODE_TITLE:
            if ram[OPER_MODE_TASK] == TITLE_MENU_TASK and not ram[DEMO_TIMER]:
                core.buttons = 0  # demo started; let it return to the title
            else:
                self._write_level(world, level)
                pressed = not pressed
                core.buttons = (1 << JOYPAD_START) if pressed else 0
            core.run(); frames += 1
        core.buttons = 0

        # 3. Run through the "WORLD x-y" intro until Mario can move.
        while frames < max_frames:
            core.run(); frames += 1
            if ram[OPER_MODE] == MODE_GAME and ram[GAME_ENGINE_SUBROUTINE] == 8:
                break
        ok = (ram[OPER_MODE] == MODE_GAME and ram[WORLD_NUMBER] == world - 1
              and ram[LEVEL_NUMBER] == level - 1)
        return ok

    def per_frame(self):
        """Keep the game going for the whole session. Returns False if it ended."""
        ram = self.ram
        if ram[OPER_MODE] == MODE_GAME and ram[NUMBER_OF_LIVES] < 3:
            ram[NUMBER_OF_LIVES] = self.LIVES
        return ram[OPER_MODE] == MODE_GAME

    def current_level(self):
        return "%d-%d" % (self.ram[WORLD_NUMBER] + 1, self.ram[LEVEL_NUMBER] + 1)

    def score(self):
        """The score as the game shows it. RAM keeps the six digits from millions down to tens; the
        ones digit is always 0 (every award is a multiple of 50) and is not stored."""
        digits = self.ram[PLAYER_SCORE_DISPLAY:PLAYER_SCORE_DISPLAY + 6]
        value = 0
        for d in digits:
            value = value * 10 + (d if d < 10 else 0)
        return value * 10
