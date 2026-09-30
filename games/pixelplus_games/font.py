"""A 3x5 pixel font and helpers to draw centred text on a small matrix."""

import numpy as np

# Each glyph is 5 rows of 3 columns; '#' is lit.
_GLYPHS = {
    "A": [".#.", "#.#", "###", "#.#", "#.#"],
    "B": ["##.", "#.#", "##.", "#.#", "##."],
    "C": [".##", "#..", "#..", "#..", ".##"],
    "D": ["##.", "#.#", "#.#", "#.#", "##."],
    "E": ["###", "#..", "##.", "#..", "###"],
    "F": ["###", "#..", "##.", "#..", "#.."],
    "G": [".##", "#..", "#.#", "#.#", ".##"],
    "H": ["#.#", "#.#", "###", "#.#", "#.#"],
    "I": ["###", ".#.", ".#.", ".#.", "###"],
    "J": ["..#", "..#", "..#", "#.#", ".#."],
    "K": ["#.#", "#.#", "##.", "#.#", "#.#"],
    "L": ["#..", "#..", "#..", "#..", "###"],
    "M": ["#.#", "###", "###", "#.#", "#.#"],
    "N": ["##.", "#.#", "#.#", "#.#", "#.#"],
    "O": [".#.", "#.#", "#.#", "#.#", ".#."],
    "P": ["##.", "#.#", "##.", "#..", "#.."],
    "Q": [".#.", "#.#", "#.#", "##.", ".##"],
    "R": ["##.", "#.#", "##.", "#.#", "#.#"],
    "S": [".##", "#..", ".#.", "..#", "##."],
    "T": ["###", ".#.", ".#.", ".#.", ".#."],
    "U": ["#.#", "#.#", "#.#", "#.#", "###"],
    "V": ["#.#", "#.#", "#.#", "#.#", ".#."],
    "W": ["#.#", "#.#", "###", "###", "#.#"],
    "X": ["#.#", "#.#", ".#.", "#.#", "#.#"],
    "Y": ["#.#", "#.#", ".#.", ".#.", ".#."],
    "Z": ["###", "..#", ".#.", "#..", "###"],
    "0": ["###", "#.#", "#.#", "#.#", "###"],
    "1": [".#.", "##.", ".#.", ".#.", "###"],
    "2": ["##.", "..#", ".#.", "#..", "###"],
    "3": ["##.", "..#", ".#.", "..#", "##."],
    "4": ["#.#", "#.#", "###", "..#", "..#"],
    "5": ["###", "#..", "##.", "..#", "##."],
    "6": [".##", "#..", "###", "#.#", "###"],
    "7": ["###", "..#", ".#.", ".#.", ".#."],
    "8": ["###", "#.#", "###", "#.#", "###"],
    "9": ["###", "#.#", "###", "..#", "##."],
    " ": ["...", "...", "...", "...", "..."],
    ".": ["...", "...", "...", "...", ".#."],
    ",": ["...", "...", "...", ".#.", "#.."],
    ":": ["...", ".#.", "...", ".#.", "..."],
    "-": ["...", "...", "###", "...", "..."],
    "_": ["...", "...", "...", "...", "###"],
    "/": ["..#", "..#", ".#.", "#..", "#.."],
    "!": [".#.", ".#.", ".#.", "...", ".#."],
    "?": ["##.", "..#", ".#.", "...", ".#."],
    "'": [".#.", ".#.", "...", "...", "..."],
    "=": ["...", "###", "...", "###", "..."],
    "+": ["...", ".#.", "###", ".#.", "..."],
    "&": [".#.", "#.#", ".#.", "#.#", ".##"],
    "#": ["#.#", "###", "#.#", "###", "#.#"],
    "%": ["#.#", "..#", ".#.", "#..", "#.#"],
    "@": ["###", "#.#", "#.#", "#..", "###"],
    "*": ["#.#", ".#.", "#.#", "...", "..."],
    "(": [".#.", "#..", "#..", "#..", ".#."],
    ")": [".#.", "..#", "..#", "..#", ".#."],
}
GLYPH_W, GLYPH_H = 3, 5
_MASKS = {c: np.array([[ch == "#" for ch in row] for row in rows], bool) for c, rows in _GLYPHS.items()}


def text_mask(text, scale=1):
    """Return a boolean mask of ``text`` rendered with 1px letter spacing."""
    text = text.upper()
    glyphs = [_MASKS.get(c, _MASKS["?"]) for c in text]
    if not glyphs:
        return np.zeros((GLYPH_H * scale, 0), bool)
    cols = []
    for i, g in enumerate(glyphs):
        if i:
            cols.append(np.zeros((GLYPH_H, 1), bool))
        cols.append(g)
    mask = np.hstack(cols)
    if scale > 1:
        mask = mask.repeat(scale, axis=0).repeat(scale, axis=1)
    return mask


def text_width(text, scale=1):
    return text_mask(text, scale).shape[1]


def best_scale(text, max_w, max_h):
    """Largest integer scale at which ``text`` fits in max_w x max_h (at least 1)."""
    s = 1
    while text_width(text, s + 1) <= max_w and GLYPH_H * (s + 1) <= max_h:
        s += 1
    return s


def draw(img, mask, x, y, color):
    """Paint ``mask`` onto ``img`` (h, w, 3) at x, y, clipping at the edges."""
    h, w = img.shape[:2]
    mh, mw = mask.shape
    x0, y0 = max(0, x), max(0, y)
    x1, y1 = min(w, x + mw), min(h, y + mh)
    if x0 >= x1 or y0 >= y1:
        return
    sub = mask[y0 - y:y1 - y, x0 - x:x1 - x]
    img[y0:y1, x0:x1][sub] = color


def draw_centered(img, text, color, y=None, scale=None):
    """Draw one line of text centred horizontally (and vertically if y is None)."""
    h, w = img.shape[:2]
    if scale is None:
        scale = best_scale(text, w, h)
    mask = text_mask(text, scale)
    x = (w - mask.shape[1]) // 2
    if y is None:
        y = (h - mask.shape[0]) // 2
    draw(img, mask, x, y, color)
    return mask.shape


def draw_lines_centered(img, lines, scale=None, gap=None):
    """Draw several (text, color) lines as a block centred on the image.

    One scale is used for every line: the largest at which all of them fit.
    """
    h, w = img.shape[:2]
    if not lines:
        return
    if scale is None:
        scale = 1
        while True:
            s = scale + 1
            g = s if gap is None else gap
            total_h = len(lines) * GLYPH_H * s + (len(lines) - 1) * g
            if total_h > h or any(text_width(t, s) > w for t, _ in lines):
                break
            scale = s
    g = scale if gap is None else gap
    total_h = len(lines) * GLYPH_H * scale + (len(lines) - 1) * g
    y = (h - total_h) // 2
    for text, color in lines:
        draw_centered(img, text, color, y=y, scale=scale)
        y += GLYPH_H * scale + g
