"""Game sessions on the matrix: timed Mario levels, and the full-time arcade."""

import logging
import os
import threading
import time

import numpy as np

from . import arcade, config, font, hat
from .audio import AudioOut
from .display import OverlayModel, Scaler
from .libretro import Core, find_core, JOYPAD_A, JOYPAD_B, JOYPAD_SELECT, JOYPAD_START
from .smb import SMB

log = logging.getLogger("pixelplus_games.game")

BANNER_SECONDS = 2.0
RESULT_SECONDS = 3.0
MASKED_BUTTONS = (1 << JOYPAD_START) | (1 << JOYPAD_SELECT)  # Mario mode: no pausing the matrix
EXIT_COMBO = (1 << JOYPAD_SELECT) | (1 << JOYPAD_START) | (1 << JOYPAD_A) | (1 << JOYPAD_B)
EXIT_HOLD_SECONDS = 2.0
ARCADE_CROP = (0, 8, 256, 232)  # the whole picture minus the lines TVs never showed


class Controls:
    """Button state shared between the web server and the emulator thread."""

    def __init__(self):
        self.buttons = 0
        self.changed = time.monotonic()  # last change, for idle detection

    def set(self, buttons):
        if buttons != self.buttons:
            self.buttons = buttons
            self.changed = time.monotonic()

    def reset(self):
        """Nothing held, and the idle clock starts now (a new player has the controller)."""
        self.buttons = 0
        self.changed = time.monotonic()


class AnyEvent:
    """Quacks like threading.Event; set when any of the given events is."""

    def __init__(self, *events):
        self.events = events

    def is_set(self):
        return any(e.is_set() for e in self.events)


SHOW_RUNNING = ("playing", "paused")


class Session:
    """Takes over the matrix (and the show) and hands both back afterwards.

    show="pause": pause a playing show and resume it afterwards (Mario).
    show="stop":  stop it; the operator starts the show again (arcade).
    """

    def __init__(self, api, cfg, show="pause"):
        if cfg.matrix is None:
            raise RuntimeError(cfg.matrix_problem or "No matrix prop selected for games")
        self.api = api
        self.cfg = cfg
        self.show = show
        self.brightness = cfg.brightness
        self.model = OverlayModel(api, cfg.matrix)
        self.paused_show = False
        # Set (from any thread) when the overlay must be opened again: pixelplusd restarted
        # (the overlay is off and its buffer no longer read) or the matrix changed size.
        self.reopen = threading.Event()

    def __enter__(self):
        self.model.open()
        state = self.api.player_state()
        if self.show == "stop" and state in SHOW_RUNNING:
            self.api.stop()
        elif self.show == "pause" and self.cfg.pause_show and state == "playing":
            self.paused_show = self.api.pause()
        self.model.blank()
        self.model.enable()
        return self

    def __exit__(self, *exc):
        try:
            if self.model.width:
                self.model.blank()
        except Exception:
            pass
        self.model.close()
        self.model.disable()
        # Resume only what we paused, and only if nobody has changed it since
        # (an operator who stopped the show meanwhile keeps it stopped).
        if self.paused_show and self.api.player_state() == "paused":
            self.api.resume()
        return False

    def reopen_model(self):
        """Map the overlay buffer again and switch the overlay back on (game thread only)."""
        self.reopen.clear()
        old = (self.model.width, self.model.height)
        try:
            self.model.open()
            self.model.enable()
        except Exception as e:  # keep playing; the next request tries again
            log.warning("Could not reopen the matrix overlay: %s", e)
            return False
        if (self.model.width, self.model.height) != old:
            log.info("Matrix is now %dx%d", self.model.width, self.model.height)
        return True

    def text(self, lines):
        img = np.zeros((self.model.height, self.model.width, 3), np.uint8)
        g = self.brightness / 100.0
        font.draw_lines_centered(img, [(t, tuple(int(v * g) for v in c)) for t, c in lines])
        self.model.write(img)


class Engine:
    def __init__(self, api):
        self.api = api
        self.core = None
        self.rom = None
        self.lock = threading.Lock()  # one emulator, one user at a time
        self.session = None           # the Session on the matrix, if any

    def request_reopen(self):
        """Ask the running session (if any) to reopen the overlay (thread-safe)."""
        session = self.session
        if session is not None:
            session.reopen.set()

    # --- setup ------------------------------------------------------------------

    def ready(self, cfg, arcade_mode=False):
        """Return None if a session can run, else the reason it can't."""
        if not find_core(config.core_path()):
            return "No NES emulator core found (install libretro-nestopia)"
        if cfg.matrix is None:
            return cfg.matrix_problem or "No matrix prop selected for games"
        if arcade_mode:
            if not arcade.list_roms(arcade.rom_folder()):
                return "No games have been uploaded for the arcade yet"
        elif not os.path.isfile(config.smb_rom_path()):
            return "No Super Mario Bros. ROM has been uploaded"
        return None

    def _load(self, cfg, rom_path):
        if self.core is None:
            os.makedirs(config.games_dir(), exist_ok=True)
            self.core = Core(find_core(config.core_path()), config.games_dir(), config.core_options())
        key = (rom_path, os.stat(rom_path).st_mtime_ns)  # a re-uploaded ROM is loaded afresh
        if self.rom != key or not self.core.game_loaded:
            self.rom = None
            self.core.load_game(rom_path)
            self.rom = key
        return self.core

    # --- the shared real-time loop ------------------------------------------------

    def _run(self, session, core, scaler, controls, stop, until=0, mask=0, on_frame=None,
             on_second=None, exit_combo=False):
        """Run the loaded game in real time at the core's frame rate.

        Returns why it ended: "time", "stop" or "exit" (the exit combo held).
        """
        cfg = session.cfg
        output_every = 1.0 / cfg.output_fps
        frame_time = 1.0 / core.fps
        audio = AudioOut(core.sample_rate, cfg.audio_device, cfg.volume)
        audio.start()
        next_output = 0.0
        next_t = time.monotonic()
        last_second = None
        combo_since = None
        reason = "time"
        try:
            while True:
                now = time.monotonic()
                if stop.is_set():
                    reason = "stop"
                    break
                if session.reopen.is_set():
                    session.reopen_model()
                    scaler.resize(session.model.width, session.model.height)
                if until and now >= until:
                    break
                buttons = controls.buttons
                if exit_combo:
                    if buttons & EXIT_COMBO == EXIT_COMBO:
                        combo_since = combo_since or now
                        if now - combo_since >= EXIT_HOLD_SECONDS:
                            reason = "exit"
                            break
                    else:
                        combo_since = None
                core.buttons = buttons & ~mask
                show = now >= next_output
                core.capture_video = show
                audio.play(core.run())
                if show and core.frame is not None:
                    next_output = max(next_output + output_every, now - output_every)
                    img = scaler.scale_frame(core.frame)
                    if on_frame:
                        on_frame(img, int(until - now + 0.999) if until else 0)
                    session.model.write(img)
                if until and on_second:
                    sec = int(until - now + 0.999)
                    if sec != last_second:
                        last_second = sec
                        on_second(sec)
                next_t += frame_time
                delay = next_t - time.monotonic()
                if delay > 0:
                    time.sleep(delay)
                elif delay < -0.25:  # fell badly behind (CPU spike): don't try to catch up
                    next_t = time.monotonic()
        finally:
            audio.stop()
            core.buttons = 0
            core.capture_video = True
        return reason

    # --- Mario mode ---------------------------------------------------------------

    def play(self, level, cfg, controls, stop, on_update):
        """One timed Super Mario Bros. level. Blocking; run on its own thread."""
        seconds = cfg.game_seconds
        score = 0
        with self.lock:
            try:
                with Session(self.api, cfg, show="pause") as session:
                    self.session = session
                    model, brightness = session.model, session.brightness
                    session.text([("WORLD", (255, 255, 255)), (level, (255, 200, 0))])
                    on_update({"phase": "starting", "level": level})
                    banner_until = time.monotonic() + BANNER_SECONDS
                    core = self._load(cfg, config.smb_rom_path())
                    smb = SMB(core)
                    snapshot = None
                    if smb.boot_to_level(level):
                        snapshot = core.serialize()  # restart point if the game ever leaves play mode
                    else:
                        log.warning("Level select for %s did not land where expected (now %s); "
                                    "is the ROM the original Super Mario Bros.?", level, smb.current_level())
                    while time.monotonic() < banner_until and not stop.is_set():
                        time.sleep(0.02)

                    scaler = Scaler(model.width, model.height, crop=cfg.crop,
                                    mode=cfg.scale_mode, brightness=brightness)
                    santa = cfg.santa_hat

                    def on_frame(img, remaining):
                        if snapshot and not smb.per_frame():
                            log.info("Game left play mode; restarting the level")
                            core.unserialize(snapshot)
                        if santa:
                            hat.draw_hat(img, core.ram, scaler, brightness)
                        if remaining and remaining <= 10:
                            draw_countdown(img, scaler, remaining, brightness)

                    def on_second(remaining):
                        on_update({"phase": "playing", "level": smb.current_level(),
                                   "remaining": remaining, "score": smb.score()})

                    reason = self._run(session, core, scaler, controls, stop,
                                       until=time.monotonic() + seconds, mask=MASKED_BUTTONS,
                                       on_frame=on_frame, on_second=on_second)
                    score = smb.score()
                    session.text([("TIME UP" if reason == "time" else "GAME OVER", (255, 255, 255)),
                                  ("%d" % score, (255, 200, 0))])
                    on_update({"phase": "over", "score": score, "level": smb.current_level()})
                    time.sleep(RESULT_SECONDS)
            except Exception as e:
                log.exception("Game session failed")
                on_update({"phase": "error", "message": str(e)})
            finally:
                self.session = None
        return score

    # --- Arcade mode ----------------------------------------------------------------

    def arcade(self, cfg, controls, stop, to_menu, on_update):
        """Full-time arcade: the game list stays on the matrix until ``stop``.

        Whoever holds the controller picks a game; holding the exit combo, or
        ``to_menu`` being set (the player's turn ended), returns to the list.
        """
        folder = arcade.rom_folder()
        with self.lock:
            try:
                with Session(self.api, cfg, show="stop") as session:
                    self.session = session
                    model = session.model
                    scaler = Scaler(model.width, model.height, crop=ARCADE_CROP,
                                    mode=cfg.scale_mode, brightness=session.brightness)
                    menu = arcade.Menu(arcade.list_roms(folder))
                    while not stop.is_set():
                        to_menu.clear()
                        on_update({"phase": "menu"})
                        choice = menu.run(session, controls, stop, folder)
                        if choice is None:
                            break
                        # A turn that ended while the list was up has already landed on the list:
                        # it must not bounce the next player straight out of the game they pick.
                        to_menu.clear()
                        name = arcade.display_name(choice)
                        session.text([("LOADING", (255, 255, 255)), (name[:20], (255, 200, 0))])
                        on_update({"phase": "arcade", "game": name})
                        try:
                            core = self._load(cfg, choice)
                            core.reset()
                        except Exception as e:
                            log.warning("Could not load %s: %s", choice, e)
                            self.rom = None
                            session.text([("CAN'T LOAD", (255, 60, 0)), (name[:20], (255, 255, 255))])
                            time.sleep(2)
                            continue
                        log.info("Arcade: playing %s", name)
                        reason = self._run(session, core, scaler, controls, AnyEvent(stop, to_menu),
                                           exit_combo=True)
                        log.info("Arcade: %s ended (%s)", name, reason)
            except Exception as e:
                log.exception("Arcade failed")
                on_update({"phase": "error", "message": str(e)})
            finally:
                self.session = None


def draw_countdown(img, scaler, remaining, brightness):
    """Last ten seconds: digits in the right-hand margin when there is one."""
    px, py, w, h = scaler.picture_rect()
    margin = img.shape[1] - (px + w)
    text = str(remaining)
    tw = font.text_width(text)
    if margin < tw + 1:
        return
    color = (255, 60, 0) if remaining <= 3 else (255, 255, 255)
    color = tuple(int(v * brightness / 100.0) for v in color)
    font.draw(img, font.text_mask(text), px + w + (margin - tw) // 2, 1, color)
