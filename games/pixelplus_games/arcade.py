"""Arcade mode: a game list on the matrix, driven from the phone controller."""

import os
import re
import time

import numpy as np

from . import config, font
from .libretro import JOYPAD_A, JOYPAD_START, JOYPAD_UP, JOYPAD_DOWN, JOYPAD_LEFT, JOYPAD_RIGHT

MAX_ROM_BYTES = 4 * 1024 * 1024
MENU_FPS = 20
REPEAT_DELAY = 0.4
REPEAT_EVERY = 0.12
MARQUEE_PX_PER_SECOND = 8
MARQUEE_PAUSE = 1.2
ROW_H = font.GLYPH_H + 1


# Friendlier names for files the web UI stores under a fixed name.
KNOWN_NAMES = {"smb.nes": "SUPER MARIO BROS."}


def rom_folder():
    """The arcade lists every .nes file in PixelPlus's ROM directory."""
    return config.rom_dir()


def list_roms(folder):
    """Sorted paths of the .nes files in ``folder``."""
    try:
        names = os.listdir(folder)
    except OSError:
        return []
    roms = []
    for n in names:
        p = os.path.join(folder, n)
        if n.lower().endswith(".nes") and os.path.isfile(p) and 16 < os.path.getsize(p) <= MAX_ROM_BYTES:
            roms.append(p)
    return sorted(roms, key=lambda p: display_name(p))


def display_name(path):
    """'Legend of Zelda, The (U) (PRG1) [!].nes' -> 'LEGEND OF ZELDA, THE'."""
    known = KNOWN_NAMES.get(os.path.basename(path).lower())
    if known:
        return known
    name = os.path.splitext(os.path.basename(path))[0]
    name = re.split(r"\s*[\(\[]", name, maxsplit=1)[0] or name
    name = name.replace("_", " ").strip()
    return name.upper()


class Menu:
    """Scrollable game list with a highlight bar and a scroll bar.

    Up/Down move (hold to repeat), Left/Right page, A or Start picks.
    """

    def __init__(self, roms):
        self.roms = roms
        self.names = [display_name(p) for p in roms]
        self.index = 0
        self.top = 0

    def set_roms(self, roms):
        current = self.roms[self.index] if self.roms else None
        self.roms = roms
        self.names = [display_name(p) for p in roms]
        self.index = roms.index(current) if current in roms else min(self.index, max(0, len(roms) - 1))

    def _layout(self, w, h):
        header_h = ROW_H + 1  # title row plus a separator line
        rows = max(1, (h - header_h) // ROW_H)
        block_h = header_h + rows * ROW_H - 1
        y0 = max(0, (h - block_h) // 2)
        return header_h, rows, y0

    def move(self, delta, rows):
        if not self.roms:
            return
        self.index = max(0, min(len(self.roms) - 1, self.index + delta))

    def render(self, w, h, brightness=100, selected_since=0.0, now=None):
        now = time.monotonic() if now is None else now
        g = brightness / 100.0
        c = lambda *rgb: tuple(int(v * g) for v in rgb)
        img = np.zeros((h, w, 3), np.uint8)
        header_h, rows, y0 = self._layout(w, h)

        # title, centred, with a separator line under it
        font.draw_centered(img, "PICK A GAME", c(255, 200, 0), y=y0, scale=1)
        img[y0 + ROW_H - 1, 1:w - 1] = c(90, 90, 90)
        if not self.roms:
            font.draw_centered(img, "NO GAMES", c(255, 60, 0), y=y0 + header_h + ROW_H, scale=1)
            return img

        # keep the selection on screen
        if self.index < self.top:
            self.top = self.index
        elif self.index >= self.top + rows:
            self.top = self.index - rows + 1
        self.top = max(0, min(self.top, max(0, len(self.roms) - rows)))

        need_bar = len(self.roms) > rows
        text_x = 1
        text_right = w - (3 if need_bar else 1)
        list_y = y0 + header_h
        for r in range(rows):
            i = self.top + r
            if i >= len(self.roms):
                break
            y = list_y + r * ROW_H
            selected = i == self.index
            if selected:
                img[y - 1:y + font.GLYPH_H, 0:text_right + 1] = c(150, 0, 0)
            mask = font.text_mask(self.names[i])
            avail = text_right - text_x
            offset = 0
            if selected and mask.shape[1] > avail:
                offset = self._marquee(mask.shape[1] - avail, now - selected_since)
            area = img[y:y + font.GLYPH_H, text_x:text_right]
            font.draw(area, mask, -offset, 0, c(255, 255, 255) if selected else c(170, 170, 170))

        if need_bar:
            track_top, track_h = list_y - 1, rows * ROW_H
            img[track_top:track_top + track_h, w - 1] = c(50, 50, 50)
            thumb_h = max(2, int(round(track_h * rows / len(self.roms))))
            span = track_h - thumb_h
            thumb_y = track_top + int(round(span * self.top / max(1, len(self.roms) - rows)))
            img[thumb_y:thumb_y + thumb_h, w - 1] = c(255, 255, 255)
        return img

    @staticmethod
    def _marquee(extra, t):
        """Scroll a too-long name back and forth, pausing at each end."""
        travel = extra / MARQUEE_PX_PER_SECOND
        cycle = 2 * (travel + MARQUEE_PAUSE)
        t = t % cycle
        if t < MARQUEE_PAUSE:
            return 0
        t -= MARQUEE_PAUSE
        if t < travel:
            return int(t * MARQUEE_PX_PER_SECOND)
        t -= travel
        if t < MARQUEE_PAUSE:
            return extra
        return max(0, extra - int((t - MARQUEE_PAUSE) * MARQUEE_PX_PER_SECOND))

    def run(self, session, controls, stop, folder):
        """Show the menu until a game is picked (returns its path) or ``stop`` is set."""
        model = session.model
        w, h = model.width, model.height
        prev = controls.buttons  # anything already held (e.g. the exit combo) is not a press
        held_since = {}
        last_repeat = {}
        selected_since = time.monotonic()
        last_scan = 0.0
        _, rows, _ = self._layout(w, h)
        while not stop.is_set():
            now = time.monotonic()
            if now - last_scan > 5:  # pick up ROMs added or removed while running
                last_scan = now
                roms = list_roms(folder)
                if roms != self.roms:
                    self.set_roms(roms)
            b = controls.buttons
            pressed = b & ~prev
            prev = b
            moved = False
            for bit, delta in ((JOYPAD_UP, -1), (JOYPAD_DOWN, 1), (JOYPAD_LEFT, -rows), (JOYPAD_RIGHT, rows)):
                mask = 1 << bit
                if pressed & mask:
                    self.move(delta, rows)
                    held_since[bit] = last_repeat[bit] = now
                    moved = True
                elif b & mask and now - held_since.get(bit, now) >= REPEAT_DELAY \
                        and now - last_repeat.get(bit, now) >= REPEAT_EVERY:
                    self.move(delta, rows)
                    last_repeat[bit] = now
                    moved = True
            if moved:
                selected_since = now
            if pressed & ((1 << JOYPAD_A) | (1 << JOYPAD_START)) and self.roms:
                return self.roms[self.index]
            model.write(self.render(w, h, session.brightness, selected_since, now))
            time.sleep(1.0 / MENU_FPS)
        return None
