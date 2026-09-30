"""Paths and settings.

Settings are not stored here: they live in PixelPlus's show.json
(``settings.games``, edited on the web UI's Games page) and are read through
``GET /api/v1/show``.  :func:`from_show` turns that document into an immutable
:class:`GameConfig`; every session takes the config as it was when it started.

Environment (all optional):

``PIXELPLUS_API``          base URL of pixelplusd (default ``http://127.0.0.1``,
                           or ``http://127.0.0.1:$PIXELPLUS_HTTP_PORT``)
``PIXELPLUS_DATA_DIR``     data directory (default ``/var/lib/pixelplus``);
                           ROMs are read from ``<data>/games/roms/``
``PIXELPLUS_GAMES_SOCKET`` control socket (default ``/run/pixelplus/games.sock``)
``PIXELPLUS_NES_CORE``     libretro core to use instead of auto-detection
``PIXELPLUS_NES_CORE_OPTIONS``  extra core options, ``key=value;key=value``
``PIXELPLUS_GAMES_DEBUG``  verbose logging when set
"""

import dataclasses
import os
from typing import Optional, Tuple


def api_base():
    base = os.environ.get("PIXELPLUS_API", "").strip()
    if base:
        return base.rstrip("/")
    port = os.environ.get("PIXELPLUS_HTTP_PORT", "").strip()
    return "http://127.0.0.1" + (":" + port if port and port != "80" else "")


def data_dir():
    return os.environ.get("PIXELPLUS_DATA_DIR", "").strip() or "/var/lib/pixelplus"


def games_dir():
    """Emulator system/save directory, and the parent of ``roms/``."""
    return os.path.join(data_dir(), "games")


def rom_dir():
    """Where the web UI stores uploaded ROMs. The arcade lists every ``*.nes`` here."""
    return os.path.join(games_dir(), "roms")


def smb_rom_path():
    """Super Mario Bros., used by the timed game."""
    return os.path.join(rom_dir(), "smb.nes")


def control_socket():
    return os.environ.get("PIXELPLUS_GAMES_SOCKET", "").strip() or "/run/pixelplus/games.sock"


def core_path():
    return os.environ.get("PIXELPLUS_NES_CORE", "").strip()


def core_options():
    opts = {}
    for part in os.environ.get("PIXELPLUS_NES_CORE_OPTIONS", "").split(";"):
        if "=" in part:
            k, v = part.split("=", 1)
            opts[k.strip()] = v.strip()
    return opts


# --- settings ---------------------------------------------------------------

INVITE_STYLES = ("text", "qr", "alternate")
DEFAULT_CROP = (8, 32, 256, 224)
DEFAULT_TURN_TIMEOUT = 20


@dataclasses.dataclass(frozen=True)
class Matrix:
    prop_id: str
    name: str
    width: int
    height: int


@dataclasses.dataclass(frozen=True)
class GameConfig:
    """``settings.games`` of the show (Rust ``GameSettings``), validated and clamped.

    Field defaults mirror ``GameSettings::default()`` in pixelplus-core.
    """

    enabled: bool = False
    matrix_prop_id: str = ""
    port: int = 8088
    game_seconds: int = 60
    cooldown_minutes: int = 5
    levels: str = ""
    play_window: str = "duringShow"      # or "anytime"
    pause_show: bool = True
    santa_hat: bool = True
    arcade_mode: bool = False
    arcade_minutes: int = 0              # 0 = unlimited turns
    arcade_idle_seconds: int = 600       # 0 = never end a turn for idling
    public_url: str = ""
    invite_every_minutes: int = 5        # 0 = only when asked (playlist command)
    invite_style: str = "text"           # text, qr, alternate
    invite_flashes: int = 3
    invite_color: Tuple[int, int, int] = (255, 0, 0)
    scale_mode: str = "fit"              # fit or stretch
    output_fps: int = 40                 # 20 or 40
    brightness: int = 100
    volume: int = 80
    crop: Tuple[int, int, int, int] = DEFAULT_CROP
    turn_timeout: int = DEFAULT_TURN_TIMEOUT
    max_queue_per_visitor: int = 3       # phones from one address in line or playing; 0 = no limit
    # from elsewhere in the show
    audio_device: str = "default"
    matrix: Optional[Matrix] = None      # the matrix prop, if one is configured and usable
    matrix_problem: Optional[str] = None  # why there is no usable matrix

    @property
    def prop_id(self):
        return self.matrix.prop_id if self.matrix else ""


def _int(d, key, default, lo=None, hi=None):
    try:
        v = int(float(d.get(key, default)))
    except (TypeError, ValueError, OverflowError):   # OverflowError: "inf", 1e999
        v = default
    if lo is not None:
        v = max(lo, v)
    if hi is not None:
        v = min(hi, v)
    return v


def _bool(d, key, default):
    v = d.get(key, default)
    if isinstance(v, str):
        return v.strip().lower() in ("1", "true", "yes", "on")
    return bool(v)


def _str(d, key, default=""):
    v = d.get(key, default)
    return v.strip() if isinstance(v, str) else default


def parse_color(value, default=(255, 0, 0)):
    """'#ff8800' / 'ff8800' / '#f80' -> (r, g, b)."""
    if not isinstance(value, str):
        return default
    v = value.strip().lstrip("#")
    if len(v) == 3:
        v = "".join(c * 2 for c in v)
    if len(v) != 6:
        return default
    try:
        return tuple(int(v[i:i + 2], 16) for i in (0, 2, 4))
    except ValueError:
        return default


def normalize_style(style, default="text"):
    """Invite style: accepts the show's names and the FPP plugin's ("both")."""
    s = str(style or "").strip().lower()
    if s == "both":
        s = "alternate"
    return s if s in INVITE_STYLES else default


def _crop(value):
    try:
        left, top, right, bottom = (int(v) for v in value)
    except (TypeError, ValueError):
        return DEFAULT_CROP
    left, top = max(0, min(255, left)), max(0, min(239, top))
    right, bottom = max(1, min(256, right)), max(1, min(240, bottom))
    if right <= left or bottom <= top:
        return DEFAULT_CROP
    return (left, top, right, bottom)


def find_matrix(show, prop_id):
    """Return (Matrix, None) or (None, reason) for the configured matrix prop.

    With no prop chosen, a show that has exactly one matrix prop uses it.
    """
    props = [p for p in (show.get("props") or []) if isinstance(p, dict)]
    if prop_id:
        prop = next((p for p in props if p.get("id") == prop_id), None)
        if prop is None:
            return None, "The matrix prop chosen for games no longer exists"
    else:
        matrices = [p for p in props if isinstance(p.get("matrix"), dict)]
        if len(matrices) != 1:
            return None, "No matrix prop selected for games"
        prop = matrices[0]
    m = prop.get("matrix")
    if not isinstance(m, dict):
        return None, 'Prop "%s" has no matrix layout' % prop.get("name", prop.get("id"))
    w, h = _int(m, "width", 0), _int(m, "height", 0)
    if w <= 0 or h <= 0:
        return None, 'Prop "%s" has no usable matrix width/height' % prop.get("name", prop.get("id"))
    return Matrix(str(prop.get("id")), str(prop.get("name") or prop.get("id")), w, h), None


def from_show(show):
    """Build a :class:`GameConfig` from a ``GET /api/v1/show`` document."""
    show = show if isinstance(show, dict) else {}
    settings = show.get("settings") if isinstance(show.get("settings"), dict) else {}
    g = settings.get("games") if isinstance(settings.get("games"), dict) else {}
    audio = settings.get("audio") if isinstance(settings.get("audio"), dict) else {}
    d = GameConfig()
    prop_id = _str(g, "matrixPropId")
    matrix, problem = find_matrix(show, prop_id)
    window = _str(g, "playWindow", d.play_window)
    return GameConfig(
        enabled=_bool(g, "enabled", d.enabled),
        matrix_prop_id=prop_id,
        port=_int(g, "port", d.port, 1, 65535),
        game_seconds=_int(g, "gameSeconds", d.game_seconds, 10, 600),
        cooldown_minutes=_int(g, "cooldownMinutes", d.cooldown_minutes, 0, 24 * 60),
        levels=_str(g, "levels"),
        play_window="anytime" if window.lower() == "anytime" else "duringShow",
        pause_show=_bool(g, "pauseShow", d.pause_show),
        santa_hat=_bool(g, "santaHat", d.santa_hat),
        arcade_mode=_bool(g, "arcadeMode", d.arcade_mode),
        arcade_minutes=_int(g, "arcadeMinutes", d.arcade_minutes, 0, 24 * 60),
        arcade_idle_seconds=_int(g, "arcadeIdleSeconds", d.arcade_idle_seconds, 0, 3600),
        public_url=_str(g, "publicUrl"),
        invite_every_minutes=_int(g, "inviteEveryMinutes", d.invite_every_minutes, 0, 24 * 60),
        invite_style=normalize_style(g.get("inviteStyle"), d.invite_style),
        invite_flashes=_int(g, "inviteFlashes", d.invite_flashes, 1, 10),
        invite_color=parse_color(g.get("inviteColor"), d.invite_color),
        scale_mode="stretch" if _str(g, "scaleMode").lower() == "stretch" else "fit",
        output_fps=20 if _int(g, "outputFps", d.output_fps) < 30 else 40,
        brightness=_int(g, "brightness", d.brightness, 0, 100),
        volume=_int(g, "volume", d.volume, 0, 100),
        crop=_crop(g.get("crop", DEFAULT_CROP)),
        turn_timeout=_int(g, "turnTimeoutSeconds", d.turn_timeout, 5, 120),
        max_queue_per_visitor=_int(g, "maxQueuePerVisitor", d.max_queue_per_visitor, 0, 100),
        audio_device=_str(audio, "device") or "default",
        matrix=matrix,
        matrix_problem=problem,
    )
