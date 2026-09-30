"""Getting pixels onto a PixelPlus matrix prop through the overlay API.

``POST /api/v1/overlay/<propId>/open`` answers ``{shm, width, height}``: a
shared-memory buffer (``/dev/shm/pixelplus-overlay-<propId>``) laid out like
FPP's overlay model buffers, so this is the plugin's display code nearly
unchanged.  Layout: a 12 byte header of three native-endian uint32s (width,
height, flags) followed by width*height RGB pixels, row-major from the
top-left.  Setting bit 0 of flags tells pixelplusd a new frame is ready; it
copies the frame on its next output pass and clears the bit.  (Bits 8-15 may
carry bytes per pixel, as in FPP; 0 means 3.)

If the shared memory is unavailable (pixelplusd on another host, an older
daemon, a permissions problem) each frame is PUT to
``/api/v1/overlay/<propId>/frame`` instead, from a sender thread that always
sends the newest frame and skips the ones that piled up.

``enable()`` / ``disable()`` switch the overlay on the prop
(``POST /api/v1/overlay/<propId> {enabled}``): while it is enabled the
overlay replaces the prop's pixels from the show.
"""

import logging
import mmap
import os
import struct
import threading

import numpy as np

from .api import ApiError

log = logging.getLogger("pixelplus_games.display")

HEADER = 12


class OverlayModel:
    def __init__(self, api, matrix):
        self.api = api
        self.matrix = matrix
        self.prop_id = matrix.prop_id
        self.name = matrix.name
        self.width = 0
        self.height = 0
        self.bpp = 3
        self.enabled = False
        self._mm = None
        self._http = False
        self._pending = None          # newest frame waiting to go out over HTTP
        self._cv = threading.Condition()
        self._sender = None
        self._sender_stop = False

    @property
    def transport(self):
        return "HTTP" if self._http else "shared memory"

    def open(self):
        """Map the prop's overlay buffer (or fall back to HTTP). Raises RuntimeError on failure."""
        self.close()
        width, height = self.matrix.width, self.matrix.height
        try:
            if not self.api.is_local():
                raise RuntimeError("pixelplusd runs on another host")
            info = self.api.overlay_open(self.prop_id)
            width = int(info.get("width") or width)
            height = int(info.get("height") or height)
            path = str(info["shm"])
            if not os.path.isabs(path):
                raise RuntimeError("unexpected shared memory path %r" % path)
            fd = os.open(path, os.O_RDWR)
            try:
                size = os.fstat(fd).st_size
                self._mm = mmap.mmap(fd, size)
            finally:
                os.close(fd)
            self.width, self.height, flags = struct.unpack_from("=III", self._mm, 0)
            self.bpp = (flags >> 8) & 0xFF or 3
            if self.bpp not in (3, 4):
                raise RuntimeError("unsupported %d bytes per pixel" % self.bpp)
            if self.width * self.height * self.bpp + HEADER > size:
                raise RuntimeError("overlay buffer smaller than its header claims")
            self._http = False
        except (OSError, ValueError, KeyError, RuntimeError, ApiError, struct.error) as e:
            log.warning("Shared memory for %s unavailable (%s); sending frames over HTTP", self.name, e)
            self.close()
            self.width, self.height, self.bpp = width, height, 3
            self._http = True
        if self.width <= 0 or self.height <= 0:
            raise RuntimeError('Matrix "%s" has no usable width/height' % self.name)
        log.info("Matrix %s: %dx%d via %s", self.name, self.width, self.height, self.transport)

    def enable(self):
        self.enabled = True
        return self.api.overlay_enable(self.prop_id, True)

    def disable(self):
        self.enabled = False
        return self.api.overlay_enable(self.prop_id, False)

    def close(self):
        if self._mm is not None:
            self._mm.close()
            self._mm = None
        if self._sender is not None:
            with self._cv:
                self._sender_stop = True  # the loop still sends the last frame (e.g. the blank)
                self._cv.notify()
            self._sender.join(timeout=3)
            self._sender = None

    def _send_loop(self):
        """HTTP mode: send the newest frame; frames that pile up are skipped."""
        while True:
            with self._cv:
                while self._pending is None and not self._sender_stop:
                    self._cv.wait()
                if self._pending is None:
                    return
                data, self._pending = self._pending, None
            self.api.overlay_frame(self.prop_id, data)

    def write(self, rgb):
        """Show an (height, width, 3) uint8 array on the matrix."""
        if self.bpp == 4:
            px = np.zeros((self.height, self.width, 4), np.uint8)
            px[..., :3] = rgb
        else:
            px = rgb
        data = np.ascontiguousarray(px, np.uint8).tobytes()
        if self._http:
            with self._cv:
                if self._sender is None:
                    self._sender_stop = False
                    self._sender = threading.Thread(target=self._send_loop, name="frames", daemon=True)
                    self._sender.start()
                self._pending = data
                self._cv.notify()
            return
        mm = self._mm
        if mm is None:
            return
        mm[HEADER:HEADER + len(data)] = data
        flags = struct.unpack_from("=I", mm, 8)[0]
        struct.pack_into("=I", mm, 8, flags | 1)

    def blank(self):
        self.write(np.zeros((self.height, self.width, 3), np.uint8))


def frame_to_rgb(frame):
    """Convert a libretro Frame into an (h, w, 3) uint8 array."""
    from .libretro import PIXEL_FORMAT_XRGB8888, PIXEL_FORMAT_RGB565

    if frame.pixel_format == PIXEL_FORMAT_XRGB8888:
        a = np.frombuffer(frame.data, np.uint8).reshape(frame.height, frame.pitch // 4, 4)
        return a[:, :frame.width, 2::-1]  # little-endian B,G,R,X -> R,G,B
    a = np.frombuffer(frame.data, "<u2").reshape(frame.height, frame.pitch // 2)[:, :frame.width]
    if frame.pixel_format == PIXEL_FORMAT_RGB565:
        r, g, b = (a >> 11) & 0x1F, (a >> 5) & 0x3F, a & 0x1F
        return np.dstack(((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))).astype(np.uint8)
    r, g, b = (a >> 10) & 0x1F, (a >> 5) & 0x1F, a & 0x1F
    return np.dstack(((r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2))).astype(np.uint8)


class Scaler:
    """Crops the NES picture, scales it down (supersampled) and centres it on the matrix.

    Crop coordinates are in the NES's 256x240 screen space.  In "stretch"
    mode the crop fills the whole model; in "fit" mode it keeps its shape
    and is centred with black bars.
    """

    def __init__(self, out_w, out_h, crop=(8, 32, 256, 224), mode="fit", brightness=100):
        self.out_w, self.out_h = out_w, out_h
        self.crop = crop
        self.mode = mode
        self.gain = max(0, min(100, brightness)) / 100.0
        self._key = None

    def _prepare(self, src_w, src_h):
        left, top, right, bottom = self.crop
        # Cores that trim overscan deliver fewer than 240 lines; keep the crop
        # anchored to the same picture content.
        dy = (240 - src_h) // 2
        dx = (256 - src_w) // 2
        top, bottom = max(0, top - dy), min(src_h, bottom - dy)
        left, right = max(0, left - dx), min(src_w, right - dx)
        if right - left < 1 or bottom - top < 1:
            left, top, right, bottom = 0, 0, src_w, src_h
        cw, ch = right - left, bottom - top

        if self.mode == "fit":
            s = min(self.out_w / cw, self.out_h / ch)
            w, h = max(1, round(cw * s)), max(1, round(ch * s))
        else:
            w, h = self.out_w, self.out_h
        self._dst = ((self.out_h - h) // 2, (self.out_w - w) // 2, h, w)
        self._src = (top, left, ch, cw)
        self._offset = (dx, dy)
        # Supersampling: each output pixel averages a k x k grid of source
        # samples spread evenly over the area it covers. Index arrays are built
        # once, so a frame costs a single gather - cheap enough for small SBCs.
        ky = max(1, min(3, int(np.ceil(ch / h))))
        kx = max(1, min(3, int(np.ceil(cw / w))))
        ys = top + np.minimum(((np.arange(h)[:, None] + (np.arange(ky)[None, :] + 0.5) / ky) * ch / h).astype(int), ch - 1)
        xs = left + np.minimum(((np.arange(w)[:, None] + (np.arange(kx)[None, :] + 0.5) / kx) * cw / w).astype(int), cw - 1)
        # shape (ky*kx, h, w): every sample of every output pixel
        self._yy = np.broadcast_to(ys.T[:, None, :, None], (ky, kx, h, w)).reshape(ky * kx, h, w)
        self._xx = np.broadcast_to(xs.T[None, :, None, :], (ky, kx, h, w)).reshape(ky * kx, h, w)
        self._n = ky * kx
        self._gain256 = int(round(self.gain * 256))
        self._key = (src_w, src_h)

    def map_point(self, x, y):
        """Map a point in NES 256x240 screen space to output pixel space (floats)."""
        if self._key is None:
            self._prepare(256, 240)
        dx, dy = self._offset
        top, left, ch, cw = self._src
        oy, ox, h, w = self._dst
        return (ox + (x - dx - left) * w / cw, oy + (y - dy - top) * h / ch)

    def picture_rect(self):
        """(x, y, w, h) of the game picture on the output, after centring."""
        if self._key is None:
            self._prepare(256, 240)
        oy, ox, h, w = self._dst
        return ox, oy, w, h

    def scale(self, rgb):
        src_h, src_w = rgb.shape[:2]
        if self._key != (src_w, src_h):
            self._prepare(src_w, src_h)
        out = np.zeros((self.out_h, self.out_w, 3), np.uint8)
        if not self._gain256:
            return out
        samples = rgb[self._yy, self._xx]                     # (n, h, w, 3) uint8
        acc = samples.sum(axis=0, dtype=np.uint32)
        acc = (acc * self._gain256 + (self._n * 128)) // (self._n * 256)
        oy, ox, h, w = self._dst
        out[oy:oy + h, ox:ox + w] = np.minimum(acc, 255)
        return out
