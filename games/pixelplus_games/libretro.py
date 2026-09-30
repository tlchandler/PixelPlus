"""Minimal headless libretro frontend built on ctypes.

Loads a libretro NES core (e.g. nestopia_libretro.so from Debian's
``libretro-nestopia`` package), runs it one frame at a time, and hands back
the video frame, the audio samples and a writable view of the console's RAM.
Nothing here is Mario-specific.
"""

import ctypes as C
import logging
import os

log = logging.getLogger("pixelplus_games.libretro")

# --- libretro constants ---------------------------------------------------

RETRO_DEVICE_JOYPAD = 1
RETRO_MEMORY_SYSTEM_RAM = 2

JOYPAD_B = 0
JOYPAD_Y = 1
JOYPAD_SELECT = 2
JOYPAD_START = 3
JOYPAD_UP = 4
JOYPAD_DOWN = 5
JOYPAD_LEFT = 6
JOYPAD_RIGHT = 7
JOYPAD_A = 8

ENV_EXPERIMENTAL = 0x10000
ENV_GET_CAN_DUPE = 3
ENV_SET_PERFORMANCE_LEVEL = 8
ENV_GET_SYSTEM_DIRECTORY = 9
ENV_SET_PIXEL_FORMAT = 10
ENV_SET_INPUT_DESCRIPTORS = 11
ENV_GET_VARIABLE = 15
ENV_SET_VARIABLES = 16
ENV_GET_VARIABLE_UPDATE = 17
ENV_GET_SAVE_DIRECTORY = 31
ENV_SET_CONTROLLER_INFO = 35
ENV_SET_GEOMETRY = 37
ENV_GET_CORE_OPTIONS_VERSION = 52

PIXEL_FORMAT_0RGB1555 = 0
PIXEL_FORMAT_XRGB8888 = 1
PIXEL_FORMAT_RGB565 = 2

# Places distributions put libretro cores.  Debian/Raspberry Pi OS use the
# multiarch directory; RetroPie and hand builds use the others.
CORE_SEARCH_DIRS = [
    "/usr/lib/aarch64-linux-gnu/libretro",
    "/usr/lib/arm-linux-gnueabihf/libretro",
    "/usr/lib/x86_64-linux-gnu/libretro",
    "/usr/lib/libretro",
    "/usr/local/lib/libretro",
    "/opt/retropie/libretrocores/lr-fceumm",
    "/opt/retropie/libretrocores/lr-nestopia",
]
CORE_NAMES = ["fceumm_libretro.so", "nestopia_libretro.so", "quicknes_libretro.so", "mesen_libretro.so"]


def find_core(preferred=""):
    """Return the path of an installed NES core, or None."""
    if preferred and os.path.isfile(preferred):
        return preferred
    for d in CORE_SEARCH_DIRS:
        for n in CORE_NAMES:
            p = os.path.join(d, n)
            if os.path.isfile(p):
                return p
    return None


# --- libretro structs -----------------------------------------------------

class retro_system_info(C.Structure):
    _fields_ = [
        ("library_name", C.c_char_p),
        ("library_version", C.c_char_p),
        ("valid_extensions", C.c_char_p),
        ("need_fullpath", C.c_bool),
        ("block_extract", C.c_bool),
    ]


class retro_game_geometry(C.Structure):
    _fields_ = [
        ("base_width", C.c_uint),
        ("base_height", C.c_uint),
        ("max_width", C.c_uint),
        ("max_height", C.c_uint),
        ("aspect_ratio", C.c_float),
    ]


class retro_system_timing(C.Structure):
    _fields_ = [("fps", C.c_double), ("sample_rate", C.c_double)]


class retro_system_av_info(C.Structure):
    _fields_ = [("geometry", retro_game_geometry), ("timing", retro_system_timing)]


class retro_game_info(C.Structure):
    _fields_ = [
        ("path", C.c_char_p),
        ("data", C.c_void_p),
        ("size", C.c_size_t),
        ("meta", C.c_char_p),
    ]


class retro_variable(C.Structure):
    _fields_ = [("key", C.c_char_p), ("value", C.c_char_p)]


ENV_CB = C.CFUNCTYPE(C.c_bool, C.c_uint, C.c_void_p)
VIDEO_CB = C.CFUNCTYPE(None, C.c_void_p, C.c_uint, C.c_uint, C.c_size_t)
AUDIO_CB = C.CFUNCTYPE(None, C.c_int16, C.c_int16)
AUDIO_BATCH_CB = C.CFUNCTYPE(C.c_size_t, C.c_void_p, C.c_size_t)
INPUT_POLL_CB = C.CFUNCTYPE(None)
INPUT_STATE_CB = C.CFUNCTYPE(C.c_int16, C.c_uint, C.c_uint, C.c_uint, C.c_uint)


class Frame:
    """One video frame as delivered by the core."""

    __slots__ = ("data", "width", "height", "pitch", "pixel_format")

    def __init__(self, data, width, height, pitch, pixel_format):
        self.data = data
        self.width = width
        self.height = height
        self.pitch = pitch
        self.pixel_format = pixel_format


class Core:
    """A loaded libretro core with one game.

    Only one Core may exist per process: libretro cores keep global state.
    """

    def __init__(self, core_path, system_dir, options=None):
        self.path = core_path
        self.lib = C.CDLL(core_path)
        self._system_dir = system_dir.encode()
        self._options = {k.encode(): v.encode() for k, v in (options or {}).items()}
        self.pixel_format = PIXEL_FORMAT_0RGB1555
        self.frame = None
        self.capture_video = True  # False skips copying frames nobody will look at
        self._audio = []
        self.buttons = 0  # bitmask of JOYPAD_* ids, read by the core
        self.game_loaded = False
        self._rom = None

        lib = self.lib
        lib.retro_api_version.restype = C.c_uint
        lib.retro_get_memory_data.restype = C.c_void_p
        lib.retro_get_memory_data.argtypes = [C.c_uint]
        lib.retro_get_memory_size.restype = C.c_size_t
        lib.retro_get_memory_size.argtypes = [C.c_uint]
        lib.retro_serialize_size.restype = C.c_size_t
        lib.retro_serialize.restype = C.c_bool
        lib.retro_serialize.argtypes = [C.c_void_p, C.c_size_t]
        lib.retro_unserialize.restype = C.c_bool
        lib.retro_unserialize.argtypes = [C.c_void_p, C.c_size_t]
        lib.retro_load_game.restype = C.c_bool
        lib.retro_load_game.argtypes = [C.POINTER(retro_game_info)]
        lib.retro_set_controller_port_device.argtypes = [C.c_uint, C.c_uint]

        if lib.retro_api_version() != 1:
            raise RuntimeError("%s: unsupported libretro API version" % core_path)

        # Callback objects must stay referenced for as long as the core lives.
        self._env_cb = ENV_CB(self._environment)
        self._video_cb = VIDEO_CB(self._video_refresh)
        self._audio_cb = AUDIO_CB(self._audio_sample)
        self._audio_batch_cb = AUDIO_BATCH_CB(self._audio_sample_batch)
        self._poll_cb = INPUT_POLL_CB(lambda: None)
        self._input_cb = INPUT_STATE_CB(self._input_state)

        lib.retro_set_environment(self._env_cb)
        lib.retro_init()
        lib.retro_set_video_refresh(self._video_cb)
        lib.retro_set_audio_sample(self._audio_cb)
        lib.retro_set_audio_sample_batch(self._audio_batch_cb)
        lib.retro_set_input_poll(self._poll_cb)
        lib.retro_set_input_state(self._input_cb)

        info = retro_system_info()
        lib.retro_get_system_info(C.byref(info))
        self.name = "%s %s" % (
            (info.library_name or b"?").decode(errors="replace"),
            (info.library_version or b"").decode(errors="replace"),
        )
        self.need_fullpath = bool(info.need_fullpath)
        log.info("Loaded libretro core %s (%s)", self.name, core_path)

    # --- callbacks --------------------------------------------------------

    def _environment(self, cmd, data):
        cmd &= ~ENV_EXPERIMENTAL
        try:
            if cmd == ENV_GET_CAN_DUPE:
                C.cast(data, C.POINTER(C.c_bool))[0] = True
                return True
            if cmd == ENV_SET_PIXEL_FORMAT:
                fmt = C.cast(data, C.POINTER(C.c_int))[0]
                if fmt in (PIXEL_FORMAT_0RGB1555, PIXEL_FORMAT_XRGB8888, PIXEL_FORMAT_RGB565):
                    self.pixel_format = fmt
                    return True
                return False
            if cmd in (ENV_GET_SYSTEM_DIRECTORY, ENV_GET_SAVE_DIRECTORY):
                C.cast(data, C.POINTER(C.c_char_p))[0] = self._system_dir
                return True
            if cmd == ENV_GET_VARIABLE:
                var = C.cast(data, C.POINTER(retro_variable))[0]
                value = self._options.get(var.key)
                if value is None:
                    return False
                # Points straight at the bytes object held in self._options,
                # which lives as long as the core does.
                C.cast(data, C.POINTER(retro_variable))[0].value = value
                return True
            if cmd == ENV_GET_VARIABLE_UPDATE:
                C.cast(data, C.POINTER(C.c_bool))[0] = False
                return True
            if cmd == ENV_GET_CORE_OPTIONS_VERSION:
                # Version 0 makes cores fall back to SET_VARIABLES, which we
                # accept and ignore; GET_VARIABLE then supplies our overrides.
                C.cast(data, C.POINTER(C.c_uint))[0] = 0
                return True
            if cmd in (ENV_SET_PERFORMANCE_LEVEL, ENV_SET_INPUT_DESCRIPTORS, ENV_SET_VARIABLES,
                       ENV_SET_CONTROLLER_INFO, ENV_SET_GEOMETRY):
                return True
        except Exception:  # never let an exception unwind into C
            log.exception("environment callback %d failed", cmd)
        return False

    def _video_refresh(self, data, width, height, pitch):
        if not data or not self.capture_video:  # dupe/unwanted frame: keep the previous one
            return
        self.frame = Frame(C.string_at(data, pitch * height), width, height, pitch, self.pixel_format)

    def _audio_sample(self, left, right):
        self._audio.append(C.string_at(C.pointer(C.c_int16 * 2)(left, right), 4))

    def _audio_sample_batch(self, data, frames):
        self._audio.append(C.string_at(data, frames * 4))
        return frames

    def _input_state(self, port, device, index, id_):
        if port != 0 or device != RETRO_DEVICE_JOYPAD:
            return 0
        return 1 if (self.buttons >> id_) & 1 else 0

    # --- API --------------------------------------------------------------

    def load_game(self, rom_path):
        self.unload_game()
        with open(rom_path, "rb") as f:
            self._rom = C.create_string_buffer(f.read())
        gi = retro_game_info()
        gi.path = rom_path.encode()
        gi.data = C.cast(self._rom, C.c_void_p)
        gi.size = len(self._rom) - 1  # create_string_buffer adds a NUL
        gi.meta = None
        if not self.lib.retro_load_game(C.byref(gi)):
            raise RuntimeError("The emulator core could not load %s" % rom_path)
        self.lib.retro_set_controller_port_device(0, RETRO_DEVICE_JOYPAD)
        av = retro_system_av_info()
        self.lib.retro_get_system_av_info(C.byref(av))
        self.fps = av.timing.fps or 60.0988
        self.sample_rate = int(round(av.timing.sample_rate or 48000))
        self.game_loaded = True

        ram_ptr = self.lib.retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM)
        ram_size = self.lib.retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM)
        self.ram = (C.c_uint8 * ram_size).from_address(ram_ptr) if ram_ptr and ram_size else None
        log.info("Game loaded: %.3f fps, %d Hz audio, %d bytes RAM exposed",
                 self.fps, self.sample_rate, ram_size if self.ram else 0)

    def unload_game(self):
        if self.game_loaded:
            self.lib.retro_unload_game()
            self.game_loaded = False
            self.ram = None
            self.frame = None
            self._audio.clear()

    def reset(self):
        self.lib.retro_reset()

    def run(self):
        """Emulate one frame. Returns the audio produced (bytes, s16le stereo)."""
        self.lib.retro_run()
        if not self._audio:
            return b""
        audio = b"".join(self._audio)
        self._audio.clear()
        return audio

    def serialize(self):
        size = self.lib.retro_serialize_size()
        buf = C.create_string_buffer(size)
        if not self.lib.retro_serialize(buf, size):
            return None
        return buf.raw

    def unserialize(self, state):
        buf = C.create_string_buffer(state, len(state))
        return bool(self.lib.retro_unserialize(buf, len(state)))
