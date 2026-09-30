"""Flash "PLAY MARIO" and the game's URL (or a QR code) on the matrix.

The show keeps running underneath; the overlay is enabled on the matrix
prop only while the invite is on screen.
"""

import logging
import time

import numpy as np

from . import font
from .display import OverlayModel

log = logging.getLogger("pixelplus_games.invite")

ON_SECONDS = 2.0
OFF_SECONDS = 0.5
SCROLL_PX_PER_SECOND = 20


def _display_url(url):
    u = url.strip()
    for prefix in ("https://", "http://"):
        if u.lower().startswith(prefix):
            u = u[len(prefix):]
    return u.rstrip("/").upper()


def qr_mask(url, max_size):
    """Boolean QR matrix (True = dark module) including its quiet zone, or None.

    Uses the widest quiet zone (2, else 1 module) that fits in max_size.
    """
    try:
        import qrcode
    except ImportError:
        log.warning("python3-qrcode is not installed; showing text instead")
        return None
    for border in (2, 1):
        qr = qrcode.QRCode(border=border, error_correction=qrcode.constants.ERROR_CORRECT_L)
        qr.add_data(url)
        qr.make(fit=True)
        m = np.array(qr.get_matrix(), bool)
        if m.shape[0] <= max_size:
            scale = max(1, max_size // m.shape[0])
            return m.repeat(scale, 0).repeat(scale, 1)
    log.warning("URL too long for a QR code on this matrix; showing text instead")
    return None


def _text_frames(w, h, url, color, gain):
    """Yield (image, seconds) for one 'on' period of the text invite."""
    title_color = tuple(int(c * gain) for c in (255, 200, 0))
    url_color = tuple(int(c * gain) for c in color)
    text = _display_url(url)
    title = "PLAY MARIO!"
    if font.text_width(text) <= w:
        img = np.zeros((h, w, 3), np.uint8)
        font.draw_lines_centered(img, [(title, title_color), (text, url_color)])
        yield img, ON_SECONDS
        return
    # URL too long for the matrix: title centred, URL scrolls across below it.
    ts = font.best_scale(title, w, h // 2)
    title_h = font.GLYPH_H * ts
    block_h = title_h + ts + font.GLYPH_H
    y_title = (h - block_h) // 2
    y_url = y_title + title_h + ts
    mask = font.text_mask(text)
    travel = w + mask.shape[1]
    steps = int(travel / SCROLL_PX_PER_SECOND * 25)
    for i in range(steps + 1):
        img = np.zeros((h, w, 3), np.uint8)
        font.draw_centered(img, title, title_color, y=y_title, scale=ts)
        font.draw(img, mask, w - int(i * travel / steps), y_url, url_color)
        yield img, 1 / 25.0


def _qr_frame(w, h, url, color, gain):
    m = qr_mask(url, min(w, h))
    if m is None:
        return None
    img = np.zeros((h, w, 3), np.uint8)
    white = int(255 * gain)
    # Normal polarity: light modules and the quiet zone lit white, dark ones off.
    qh, qw = m.shape
    x0, y0 = (w - qw) // 2, (h - qh) // 2
    side = w - qw
    if side >= 2 * (font.text_width("SCAN") + 2):
        # room either side: put the QR on the left third and a caption on the right
        x0 = (w // 2 - qw) // 2 if w // 2 >= qw else x0
        cap = img[:, x0 + qw + 1:]
        font.draw_lines_centered(cap, [("SCAN", tuple(int(c * gain) for c in color)),
                                       ("TO", tuple(int(c * gain) for c in color)),
                                       ("PLAY", tuple(int(c * gain) for c in color))], scale=1)
    img[y0:y0 + qh, x0:x0 + qw][~m] = white
    return img


def show(api, cfg, url, flashes=3, style="text"):
    """Blocking: flash the invite ``flashes`` times. style: text, qr or alternate."""
    model = OverlayModel(api, cfg.matrix)
    gain = cfg.brightness / 100.0
    color = cfg.invite_color
    flashes = max(1, min(10, int(flashes)))
    try:
        model.open()
        model.blank()
        model.enable()
        link = url if "://" in url else "https://" + url
        qr_img = _qr_frame(model.width, model.height, link, color, gain) if style in ("qr", "alternate") else None
        for i in range(flashes):
            if qr_img is not None and (style == "qr" or i % 2 == 1):
                model.write(qr_img)
                time.sleep(ON_SECONDS * 2)
            else:
                for img, secs in _text_frames(model.width, model.height, url, color, gain):
                    model.write(img)
                    time.sleep(secs)
            model.blank()
            time.sleep(OFF_SECONDS)
    except Exception:
        log.exception("Invite failed")
        raise
    finally:
        _release(model)


def test_pattern(api, cfg, seconds=6):
    """Border, corner markers and centred text, so orientation is easy to check."""
    model = OverlayModel(api, cfg.matrix)
    gain = cfg.brightness / 100.0
    try:
        model.open()
        w, h = model.width, model.height
        img = np.zeros((h, w, 3), np.uint8)
        c = lambda *rgb: tuple(int(v * gain) for v in rgb)
        img[0, :], img[-1, :], img[:, 0], img[:, -1] = c(0, 0, 255), c(0, 0, 255), c(0, 0, 255), c(0, 0, 255)
        img[:3, :3] = c(255, 0, 0)        # top-left is red
        img[:3, -3:] = c(0, 255, 0)       # top-right is green
        font.draw_lines_centered(img[2:-2, 2:-2], [("MARIO", c(255, 0, 0)), ("%dX%d" % (w, h), c(255, 255, 255))])
        model.write(img)
        model.enable()
        time.sleep(seconds)
    except Exception:
        log.exception("Test pattern failed")
        raise
    finally:
        _release(model)


def _release(model):
    """Blank the matrix and hand it back to the show."""
    try:
        if model.width:
            model.blank()
    except Exception:
        pass
    model.close()
    model.disable()
