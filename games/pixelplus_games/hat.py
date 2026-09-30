"""A Santa hat drawn on Mario, at the matrix's own resolution.

Mario's NES sprite has only three colours (no white), so instead of editing
the ROM's graphics the hat is painted onto each scaled frame, positioned from
the game's RAM.  Drawing after scaling keeps it crisp instead of letting the
box filter smear the white trim into pink.
"""

import numpy as np

# RAM (SMBDIS.ASM)
PLAYER_REL_XPOS = 0x03AD      # left edge of Mario's 16px-wide sprite, screen space
PLAYER_REL_YPOS = 0x03B8      # top of his 32px-tall sprite box
PLAYER_Y_HIGHPOS = 0x00B5     # 1 while he is on the visible screen
PLAYER_FACING_DIR = 0x0033    # 1 right, 2 left
PLAYER_SIZE = 0x0754          # 0 big, 1 small
CROUCHING_FLAG = 0x0714
PLAYER_OFFSCREEN_BITS = 0x03D0
GAME_ENGINE_SUBROUTINE = 0x000E
OPER_MODE = 0x0770

# Facing right; mirrored when he faces left so the pom-pom always trails.
#   W = white, R = red, . = leave the game pixel alone.
HAT = [
    "W R . .",
    ". R R .",
    "W W W W",   # the trim, which sits on the top row of his head
]
_COLORS = {"W": (255, 255, 255), "R": (230, 0, 0)}


def _pattern(facing_left):
    rows = [r.split() for r in HAT]
    if facing_left:
        rows = [list(reversed(r)) for r in rows]
    return rows


def draw_hat(img, ram, scaler, brightness=100):
    """Paint the hat onto ``img`` (the scaled frame) if Mario is visible."""
    if ram[OPER_MODE] != 1 or ram[PLAYER_Y_HIGHPOS] != 1:
        return
    if ram[GAME_ENGINE_SUBROUTINE] in (0x00, 0x02, 0x03, 0x06):  # entering/leaving via pipes, dead
        return
    if ram[PLAYER_OFFSCREEN_BITS] & 0x0F:  # partly off the left/right edge
        return

    x = int(ram[PLAYER_REL_XPOS])
    y = int(ram[PLAYER_REL_YPOS])
    if ram[PLAYER_SIZE] == 1:
        head_top = y + 16          # small Mario uses the bottom half of the box
    elif ram[CROUCHING_FLAG]:
        head_top = y + 8
    else:
        head_top = y
    # NES sprites are drawn one line below their Y coordinate.
    head_top += 1

    cx, top = scaler.map_point(x + 8, head_top)
    x0, _ = scaler.map_point(x, head_top)
    x1, _ = scaler.map_point(x + 16, head_top)
    mario_w = x1 - x0
    k = max(1, int(round(mario_w / 4.0)))  # hat pixel size for bigger matrices

    pat = _pattern(ram[PLAYER_FACING_DIR] == 2)
    pw = len(pat[0]) * k
    left = int(round(cx - pw / 2.0))
    trim_row = int(np.floor(top + 0.25))  # the output row holding his hair/cap
    top_row = trim_row - (len(pat) - 1) * k

    px, py, w, h = scaler.picture_rect()
    gain = max(0, min(100, brightness)) / 100.0
    for r, row in enumerate(pat):
        for c, ch in enumerate(row):
            color = _COLORS.get(ch)
            if color is None:
                continue
            ys, xs = top_row + r * k, left + c * k
            y0, y1 = max(py, ys), min(py + h, ys + k)
            x0_, x1_ = max(px, xs), min(px + w, xs + k)
            if y0 < y1 and x0_ < x1_:
                img[y0:y1, x0_:x1_] = [int(v * gain) for v in color]
