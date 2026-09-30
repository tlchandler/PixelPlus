"""PixelPlus games sidecar: phone controller web server + game queue.

Run as ``python3 -m pixelplus_games`` (``pixelplus-games.service``).  The
settings come from the show (``settings.games``, the web UI's Games page).
The phone controller port is open only while games are enabled; the local
control socket (used by pixelplusd for ``/api/v1/games/*``) is always there.

Two modes:
  Mario  - during the show, a visitor plays a random SMB level for a minute,
           the show pauses and resumes, then a cooldown.
  Arcade - full time: the show is stopped and kept stopped, the matrix shows
           a list of NES games, and visitors take turns playing any of them.
"""

import asyncio
import json
import logging
import os
import random
import secrets
import signal
import sys
import threading
import time

from . import config, invite
from .api import ApiError, PixelPlus
from .events import EventStream
from .game import SHOW_RUNNING, Controls, Engine
from .smb import parse_levels
from .web import Server

log = logging.getLogger("pixelplus_games.server")

HERE = os.path.dirname(os.path.abspath(__file__))
PAGE_PATH = os.path.join(HERE, "www", "controller.html")
RECONNECT_GRACE = 15      # seconds a queued/active player may drop and come back
ABANDON_AFTER = 10        # end a game early if its player has been gone this long
BUTTON_MASK = 0x1FF       # libretro joypad ids 0..8
REPLACED = 4001           # websocket close code: this player connected again elsewhere
SHOW_POLL_LIVE = 60       # refetch /show this often even with live updates (safety net)
SHOW_POLL = 5             # ... and this often when the event stream is down
STATUS_STALE = 5          # poll /player when no status event arrived for this long
KEEP_STOPPED_EVERY = 5    # arcade: seconds between attempts to stop a show that started


class Client:
    def __init__(self, cid, ws):
        self.id = cid
        self.ws = ws
        self.last_seen = time.monotonic()
        self.msg_times = []


class Hub:
    def __init__(self, loop, api=None):
        self.loop = loop
        self.api = api or PixelPlus(config.api_base())
        self.cfg = config.GameConfig()
        self.show_loaded = False
        self.show_version = None
        self.engine = Engine(self.api)
        self.clients = {}           # id -> Client (most recent connection)
        self.queue = []             # ids waiting, in order
        self.gone = {}              # id -> time it disconnected
        self.active = None          # id of the player
        self.offer = None           # (id, deadline) when the queue head is being offered a turn
        self.cooldown_until = 0
        self.controls = Controls()
        self.stop_event = threading.Event()
        self.game_thread = None
        self.game_view = {}
        self.last_result = {}       # id -> result shown after a game
        self.player_status = {}     # latest PlayerStatus from pixelplusd
        self.player_state = "unknown"
        self.status_at = 0.0        # when player_status was last updated
        self.busy_reason = None     # e.g. an invite is showing
        self.last_invite = time.monotonic()
        self.last_error = None
        self._was_cooling = False
        # arcade mode
        self.arcade_thread = None
        self.arcade_stop = threading.Event()
        self.to_menu = threading.Event()
        self.arcade_view = {}
        self.turn_started = 0
        self.last_show_stop = 0
        # settings refresh
        self.show_fetched = 0.0
        self._refresh_task = None
        self._refresh_again = False
        self.events = None
        self.server = None
        self.server_port = None
        self._page = None
        self._page_mtime = 0

    # --- settings / pixelplusd state ------------------------------------------------

    def page(self):
        try:
            m = os.stat(PAGE_PATH).st_mtime
            if self._page is None or m != self._page_mtime:
                with open(PAGE_PATH, "rb") as f:
                    self._page = f.read()
                self._page_mtime = m
        except OSError:
            self._page = b"<h1>Controller page missing</h1>"
        return self._page

    def apply_show(self, show):
        """Adopt the games settings from a Show document."""
        cfg = config.from_show(show)
        first = not self.show_loaded
        self.show_loaded = True
        self.show_version = show.get("version")
        if cfg == self.cfg and not first:
            return
        old, self.cfg = self.cfg, cfg
        log.info("Settings %s: games %s, matrix %s, %s mode", "loaded" if first else "changed",
                 "on" if cfg.enabled else "off",
                 "%s (%dx%d)" % (cfg.matrix.name, cfg.matrix.width, cfg.matrix.height) if cfg.matrix
                 else "none (%s)" % cfg.matrix_problem,
                 "arcade" if cfg.arcade_mode else "Mario")
        if first and cfg.matrix and not self.game_running() and not self.busy_reason:
            # A previous run may have died with the overlay on: hand the prop back to the show.
            self.loop.run_in_executor(None, self.api.overlay_enable, cfg.matrix.prop_id, False)
        elif old.matrix and old.matrix != cfg.matrix and not self.game_running() and not self.arcade_running():
            self.loop.run_in_executor(None, self.api.overlay_enable, old.matrix.prop_id, False)
        self.push_all()

    async def refresh_show(self):
        """Fetch /show and apply it. Concurrent requests collapse into one extra fetch."""
        if self._refresh_task and not self._refresh_task.done():
            self._refresh_again = True
            return await asyncio.shield(self._refresh_task)
        self._refresh_task = asyncio.ensure_future(self._refresh_loop())
        return await asyncio.shield(self._refresh_task)

    async def _refresh_loop(self):
        while True:
            self._refresh_again = False
            self.show_fetched = time.monotonic()
            try:
                show = await self.loop.run_in_executor(None, self.api.show)
            except ApiError as e:
                if self.show_loaded:
                    log.warning("Could not refresh settings: %s", e)
                else:
                    log.warning("Waiting for pixelplusd: %s", e)
                self._set_api_error(str(e))
                return False
            self.apply_show(show)
            if self.last_error and self.last_error.startswith("pixelplusd"):
                self.last_error = None
            if not self._refresh_again:
                return True

    def _set_api_error(self, message):
        self.last_error = message if message.startswith("pixelplusd") else "pixelplusd: " + message

    def on_event(self, kind, data):
        """Live update from pixelplusd's WebSocket (runs on the event loop)."""
        if kind == "show":
            version = data.get("version") if isinstance(data, dict) else None
            if version is None or version != self.show_version:
                asyncio.ensure_future(self.refresh_show())
        elif kind == "status" and isinstance(data, dict):
            self._set_player_status(data)

    def _set_player_status(self, status):
        state = status.get("state", "unknown")
        changed = state != self.player_state
        self.player_status = status
        self.player_state = state
        self.status_at = time.monotonic()
        if changed:
            log.debug("Show state: %s", state)
        return changed

    def show_on(self):
        """True while the show is on: playing, paused (e.g. by us), or inside a schedule window."""
        return self.player_state in SHOW_RUNNING or bool(self.player_status.get("scheduleEntry"))

    # --- configuration / availability --------------------------------------

    def cooldown_left(self):
        return max(0, int(self.cooldown_until - time.monotonic() + 0.999))

    def arcade_mode(self):
        return self.cfg.enabled and self.cfg.arcade_mode

    def arcade_running(self):
        return self.arcade_thread is not None and self.arcade_thread.is_alive()

    def unavailable_reason(self):
        if not self.show_loaded:
            return "Starting up. Try again in a moment!"
        if not self.cfg.enabled:
            return "Games are turned off right now."
        if self.arcade_mode():
            reason = self.engine.ready(self.cfg, arcade_mode=True)
            if not reason and not self.arcade_running():
                reason = "The arcade is starting up..."
            return reason
        reason = self.engine.ready(self.cfg)
        if reason:
            return reason
        if self.cooldown_left() and not self.game_running():
            return "cooldown"
        if self.cfg.play_window == "duringShow" and not self.show_on() and not self.game_running():
            return "The show is taking a break. Come back while the lights are running!"
        return None

    def game_running(self):
        return self.game_thread is not None and self.game_thread.is_alive()

    # --- per-client view ----------------------------------------------------

    def view(self, cid):
        waiting = self.queue.index(cid) + 1 if cid in self.queue else 0
        base = {"t": "state", "queue": len(self.queue), "seconds": self.cfg.game_seconds,
                "arcade": self.arcade_mode()}
        if cid == self.active and self.arcade_mode():
            v = dict(base, **self.arcade_view)
            left = self.turn_left()
            if left is not None:
                v["remaining"] = left
            return v
        if cid == self.active:
            v = dict(base, **self.game_view)
            v.setdefault("phase", "starting")
            return v
        if cid in self.last_result:
            return dict(base, **self.last_result[cid])
        if self.offer and self.offer[0] == cid:
            return dict(base, phase="yourturn", timeout=max(0, int(self.offer[1] - time.monotonic())))
        if waiting:
            return dict(base, phase="queued", position=waiting)
        reason = self.unavailable_reason()
        if reason == "cooldown":
            return dict(base, phase="cooldown", wait=self.cooldown_left())
        if reason:
            return dict(base, phase="closed", message=reason)
        if self.active or self.game_running() or self.offer or self.queue:
            remaining = self.turn_left() if self.arcade_mode() else self.game_view.get("remaining")
            return dict(base, phase="watching", remaining=remaining)
        return dict(base, phase="idle")

    def push(self, cid):
        c = self.clients.get(cid)
        if c and not c.ws.closed:
            c.ws.send_json(self.view(cid))

    def push_all(self):
        for cid in list(self.clients):
            self.push(cid)

    # --- game flow ------------------------------------------------------------

    def press_start(self, cid):
        self.last_result.pop(cid, None)
        if cid == self.active:
            return
        if self.unavailable_reason():
            self.push(cid)
            return
        if self.offer and self.offer[0] == cid:
            self.offer = None
            self.queue.remove(cid)
            self.start_game(cid)
        elif not self.active and not self.game_running() and not self.offer and not self.queue \
                and not self.busy_reason:
            self.start_game(cid)
        elif cid not in self.queue:
            self.queue.append(cid)
        self.push_all()

    def start_game(self, cid):
        if self.arcade_mode():
            self.start_turn(cid)
            return
        level = random.choice(parse_levels(self.cfg.levels))
        log.info("Starting game for player %s on level %s", cid[:6], level)
        self.active = cid
        self.controls.reset()
        self.stop_event.clear()
        self.game_view = {"phase": "starting", "level": level}
        cfg = self.cfg

        def update(d):
            self.loop.call_soon_threadsafe(self._game_update, cid, d)

        def run():
            try:
                self.engine.play(level, cfg, self.controls, self.stop_event, update)
            finally:
                self.loop.call_soon_threadsafe(self._game_done, cid)

        self.game_thread = threading.Thread(target=run, name="game", daemon=True)
        self.game_thread.start()

    def _game_update(self, cid, d):
        if d.get("phase") == "error":
            self.last_error = d.get("message") or "The game failed"
        elif d.get("phase") == "playing" and self.last_error and not self.last_error.startswith("pixelplusd"):
            self.last_error = None
        if cid != self.active:
            return
        self.game_view.update(d)
        if d.get("phase") in ("over", "error"):
            self.controls.buttons = 0
            self.last_result[cid] = dict(self.game_view)
        self.push(cid)
        # people waiting see the clock
        for other in list(self.clients):
            if other != cid and other not in self.last_result:
                self.push(other)

    def _game_done(self, cid):
        log.info("Game finished for player %s", cid[:6])
        self.active = None
        self.game_thread = None
        self.game_view = {}
        self.controls.buttons = 0
        cooldown = self.cfg.cooldown_minutes * 60
        self.cooldown_until = time.monotonic() + cooldown
        if cooldown:
            # Nobody plays until the cooldown is over, so there is no line to keep:
            # everyone who was waiting sees the countdown instead.
            self.queue = []
            self.offer = None
            log.info("Cooldown for %d minutes", cooldown // 60)
        self.push_all()

    def stop_game(self):
        stopped = False
        if self.game_running():
            self.stop_event.set()
            stopped = True
        if self.arcade_running() and self.active:
            self.end_turn("Your turn was ended by the show operator.")
            stopped = True
        return stopped

    # --- arcade turns -------------------------------------------------------------

    def turn_left(self):
        minutes = self.cfg.arcade_minutes
        if not minutes or not self.active:
            return None
        return max(0, int(self.turn_started + minutes * 60 - time.monotonic() + 0.999))

    def start_turn(self, cid):
        log.info("Arcade: player %s takes the controller", cid[:6])
        self.active = cid
        self.turn_started = time.monotonic()
        self.controls.reset()  # the idle clock starts with this player, not the last one
        self.push_all()

    def end_turn(self, why):
        cid = self.active
        if not cid:
            return
        log.info("Arcade: player %s's turn is over (%s)", cid[:6], why)
        self.active = None
        self.controls.set(0)
        self.to_menu.set()  # whatever they were playing, back to the game list
        self.last_result[cid] = {"phase": "over", "message": why}
        self.push_all()

    def _arcade_update(self, d):
        if d.get("phase") == "error":
            self.last_error = d.get("message") or "The arcade failed"
        self.arcade_view = d
        if self.active:
            self.push(self.active)

    def _manage_arcade(self, now):
        """Start/stop the full-time arcade to match the setting; police turns."""
        want = self.arcade_mode() and not self.engine.ready(self.cfg, arcade_mode=True)
        if want and not self.arcade_running():
            if self.game_running():
                self.stop_event.set()  # a Mario game is on; the arcade starts once it has ended
                return
            if self.busy_reason:
                return
            log.info("Arcade mode on: stopping the show and opening the arcade")
            self.queue, self.offer, self.cooldown_until = [], None, 0
            self.arcade_stop.clear()
            self.to_menu.clear()
            self.arcade_view = {}
            self.last_show_stop = now
            cfg = self.cfg

            def update(d):
                self.loop.call_soon_threadsafe(self._arcade_update, d)

            self.arcade_thread = threading.Thread(
                target=self.engine.arcade, args=(cfg, self.controls, self.arcade_stop, self.to_menu, update),
                name="arcade", daemon=True)
            self.arcade_thread.start()
            self.push_all()
            return
        if not want and self.arcade_running():
            log.info("Arcade mode off: closing the arcade (start the show again from PixelPlus)")
            if self.active:
                self.end_turn("The arcade has closed.")
            self.arcade_stop.set()
            self.queue, self.offer = [], None
            self.push_all()
            return
        if not self.arcade_running():
            return
        # keep the show stopped: the scheduler would otherwise restart it
        if self.player_state in SHOW_RUNNING and now - self.last_show_stop > KEEP_STOPPED_EVERY:
            self.last_show_stop = now
            log.info("Arcade mode: stopping a show that started (turn Arcade mode off to run the show)")
            self.loop.run_in_executor(None, self.api.stop)
        if self.active:
            left = self.turn_left()
            idle = self.cfg.arcade_idle_seconds
            if left == 0:
                self.end_turn("Time's up! Thanks for playing.")
            elif idle and now - self.controls.changed > idle:
                self.end_turn("Your turn ended because nothing was pressed for a while.")
            elif left is not None:
                self.push(self.active)  # the phone shows the turn clock

    def _offer_next(self):
        now = time.monotonic()
        if self.offer and now > self.offer[1]:
            cid = self.offer[0]
            log.info("Player %s missed their turn", cid[:6])
            if cid in self.queue:
                self.queue.remove(cid)
            self.offer = None
            self.push(cid)
        if self.offer or self.active or self.game_running() or self.busy_reason:
            return
        # drop queued players who left and did not come back
        self.queue = [q for q in self.queue if q in self.clients or now - self.gone.get(q, now) < RECONNECT_GRACE]
        if self.queue and not self.unavailable_reason():
            cid = self.queue[0]
            self.offer = (cid, now + self.cfg.turn_timeout)
            self.push_all()

    # --- websocket clients ------------------------------------------------------

    async def handle_ws(self, ws):
        client = None
        try:
            while True:
                text = await ws.recv()
                if text is None:
                    break
                try:
                    msg = json.loads(text)
                    kind = msg.get("t")
                except (ValueError, AttributeError):
                    continue
                if client is None:
                    if kind != "hello":
                        continue
                    cid = str(msg.get("id", ""))[:64]
                    if len(cid) < 16 or not cid.replace("-", "").isalnum():
                        cid = secrets.token_hex(16)
                    old = self.clients.get(cid)
                    if old and old.ws is not ws:
                        # Same player opened the page again (another tab, the home-screen icon...).
                        # 4001 tells the old page it was replaced, so it waits for a tap instead of
                        # reconnecting and kicking this one straight back out.
                        await old.ws.close(REPLACED)
                    client = Client(cid, ws)
                    self.clients[cid] = client
                    self.gone.pop(cid, None)
                    ws.send_json({"t": "welcome", "id": cid})
                    self.push(cid)
                    continue
                client.last_seen = time.monotonic()
                # simple flood control: at most 60 messages per second
                now = client.last_seen
                client.msg_times = [t for t in client.msg_times if now - t < 1.0]
                client.msg_times.append(now)
                if len(client.msg_times) > 60:
                    continue
                if kind == "in" and client.id == self.active:
                    try:
                        self.controls.set(int(msg.get("b", 0)) & BUTTON_MASK)
                    except (TypeError, ValueError):
                        pass
                elif kind == "start":
                    self.press_start(client.id)
                elif kind == "leave":
                    if client.id in self.queue:
                        self.queue.remove(client.id)
                    if self.offer and self.offer[0] == client.id:
                        self.offer = None
                    self.push_all()
                elif kind == "ack":
                    self.last_result.pop(client.id, None)
                    self.push(client.id)
        finally:
            if client and self.clients.get(client.id) is client:
                del self.clients[client.id]
                self.gone[client.id] = time.monotonic()
                if client.id == self.active:
                    self.controls.set(0)

    # --- housekeeping -----------------------------------------------------------

    async def tick(self):
        """Once a second: refresh settings and show state, advance the queue."""
        while True:
            try:
                await self._tick_once()
            except Exception:
                log.exception("housekeeping failed")
            await asyncio.sleep(1)

    async def _tick_once(self):
        now = time.monotonic()
        live = self.events is not None and self.events.connected
        if not self.show_loaded or now - self.show_fetched >= (SHOW_POLL_LIVE if live else SHOW_POLL):
            await self.refresh_show()

        await self._update_server()

        changed = False
        if not live or now - self.status_at > STATUS_STALE:
            status = await self.loop.run_in_executor(None, self.api.player)
            changed = self._set_player_status(status if isinstance(status, dict) else {"state": "unknown"})

        now = time.monotonic()
        if self.active and self.active not in self.clients:
            gone_at = self.gone.get(self.active, now)
            if now - gone_at > ABANDON_AFTER and not self.stop_event.is_set():
                log.info("Player left; ending their game early")
                if self.arcade_running():
                    self.end_turn("You left the page.")
                else:
                    self.stop_game()
        for cid, t in list(self.gone.items()):
            if now - t > 300:
                del self.gone[cid]
                self.last_result.pop(cid, None)
        in_cooldown = self.cooldown_left() > 0
        if self._was_cooling and not in_cooldown:
            changed = True  # cooldown just ended: phones go back to PRESS START
        self._was_cooling = in_cooldown
        self._manage_arcade(now)
        self._offer_next()
        self._auto_invite(now)
        if changed or self.offer:
            self.push_all()

    async def _update_server(self):
        """The controller page runs only while games are enabled."""
        want_port = self.cfg.port if self.cfg.enabled else None
        if want_port == self.server_port:
            return
        if self.server:
            await self.server.stop()
            self.server = None
            log.info("Controller page stopped")
        if want_port:
            srv = Server(self.page, self.handle_ws)
            try:
                await srv.start(want_port)
                self.server = srv
            except OSError as e:
                log.error("Could not listen on port %d: %s", want_port, e)
                self.last_error = "Could not open the controller port %d: %s" % (want_port, e.strerror or e)
        self.server_port = want_port if self.server else None

    def _auto_invite(self, now):
        """Flash the invite every few minutes while games are possible."""
        every = self.cfg.invite_every_minutes * 60
        if not every or not self.cfg.enabled or not self.cfg.public_url or self.arcade_mode():
            return
        if now - self.last_invite < every:
            return
        if self.game_running() or self.queue or self.offer or self.busy_reason:
            return  # the matrix is busy; try again next second
        if self.unavailable_reason():
            return  # cooldown, or no show running: don't invite people to something closed
        self.last_invite = now
        self.start_invite({})

    # --- local control socket (pixelplusd's /api/v1/games/*) ----------------------

    def status(self):
        cfg = self.cfg
        running = self.game_running() or self.arcade_running()
        reply = {
            "ok": True,
            "enabled": cfg.enabled,
            "running": running,
            "arcade": self.arcade_running(),
            "queueLength": len(self.queue),
            "cooldownS": self.cooldown_left(),
            "model": {"width": cfg.matrix.width, "height": cfg.matrix.height} if cfg.matrix else None,
            "propId": cfg.prop_id or None,
            "clients": len(self.clients),
            "port": self.server_port,
            "showState": self.player_state,
            "busy": self.busy_reason,
            "unavailable": self.unavailable_reason(),
        }
        if self.active:
            view = self.arcade_view if self.arcade_running() else self.game_view
            player = {"phase": view.get("phase", "starting")}
            for k in ("level", "score", "game", "remaining"):
                if view.get(k) is not None:
                    player[k] = view[k]
            if self.arcade_running():
                left = self.turn_left()
                if left is not None:
                    player["remaining"] = left
            reply["player"] = player
        if self.last_error:
            reply["lastError"] = self.last_error
        return reply

    async def handle_control(self, reader, writer):
        try:
            line = await asyncio.wait_for(reader.readline(), 5)
            req = json.loads(line or b"{}")
            if not isinstance(req, dict):
                raise ValueError("expected a JSON object")
            cmd = req.get("cmd")
            if cmd == "status":
                reply = self.status()
            elif cmd == "stop":
                reply = {"ok": True, "stopped": self.stop_game()}
            elif cmd == "invite":
                reply = self.start_invite(req)
            elif cmd == "test":
                reply = self.start_test()
            elif cmd == "reload":
                reply = {"ok": bool(await self.refresh_show())}
                if not reply["ok"]:
                    reply["error"] = self.last_error or "could not read the show"
            else:
                reply = {"ok": False, "error": "unknown command %r" % (cmd,)}
        except Exception as e:
            reply = {"ok": False, "error": str(e) or e.__class__.__name__}
        writer.write((json.dumps(reply) + "\n").encode())
        try:
            await writer.drain()
        except (ConnectionError, OSError):
            pass
        finally:
            writer.close()

    def _matrix_busy(self):
        if self.game_running() or self.arcade_running():
            return "a game is on the matrix"
        if self.busy_reason:
            return "the matrix is busy (%s)" % self.busy_reason
        return None

    def _run_on_matrix(self, reason, target, *args):
        """Run a blocking matrix job (invite, test pattern) on its own thread."""
        self.busy_reason = reason

        def run():
            try:
                target(*args)
            except Exception as e:
                msg = "%s failed: %s" % (reason.capitalize(), e)
                self.loop.call_soon_threadsafe(setattr, self, "last_error", msg)
            finally:
                self.loop.call_soon_threadsafe(setattr, self, "busy_reason", None)

        threading.Thread(target=run, name=reason, daemon=True).start()

    def start_test(self):
        """Show a centred test pattern for a few seconds (Games page button)."""
        busy = self._matrix_busy()
        if busy:
            return {"ok": False, "error": busy}
        if self.cfg.matrix is None:
            return {"ok": False, "error": self.cfg.matrix_problem or "no matrix prop selected"}
        self._run_on_matrix("test", invite.test_pattern, self.api, self.cfg)
        return {"ok": True}

    def start_invite(self, req):
        busy = self._matrix_busy()
        if busy:
            return {"ok": False, "error": busy}
        if not self.cfg.enabled and not req.get("force"):
            return {"ok": False, "error": "games are turned off"}
        if self.cooldown_left() and not req.get("force"):
            return {"ok": False, "error": "cooldown: %d seconds left" % self.cooldown_left()}
        url = str(req.get("url") or self.cfg.public_url).strip()
        if not url:
            return {"ok": False, "error": "set the Public URL on the Games page"}
        if self.cfg.matrix is None:
            return {"ok": False, "error": self.cfg.matrix_problem or "no matrix prop selected"}
        try:
            flashes = int(req.get("flashes") or 0)
        except (TypeError, ValueError):
            flashes = 0
        flashes = max(1, min(10, flashes or self.cfg.invite_flashes))
        style = config.normalize_style(req.get("style"), self.cfg.invite_style)
        self.last_invite = time.monotonic()
        log.info("Showing invite (%s, %d flashes)", style, flashes)
        self._run_on_matrix("invite", invite.show, self.api, self.cfg, url, flashes, style)
        return {"ok": True}


# --- process -----------------------------------------------------------------------


def setup_logging():
    level = logging.DEBUG if os.environ.get("PIXELPLUS_GAMES_DEBUG") else logging.INFO
    # systemd adds its own timestamps to the journal
    fmt = "%(levelname)s %(name)s: %(message)s" if os.environ.get("JOURNAL_STREAM") \
        else "%(asctime)s %(levelname)s %(name)s: %(message)s"
    logging.basicConfig(level=level, stream=sys.stderr, format=fmt)


async def open_control_socket(hub, path):
    directory = os.path.dirname(path)
    try:
        os.makedirs(directory, exist_ok=True)
    except OSError as e:
        fallback = os.path.join(config.games_dir(), "games.sock")
        log.warning("Cannot create %s (%s); using %s. Set PIXELPLUS_GAMES_SOCKET for both "
                    "pixelplusd and this service.", directory, e, fallback)
        path = fallback
        os.makedirs(os.path.dirname(path), exist_ok=True)
    try:
        os.unlink(path)
    except OSError:
        pass
    server = await asyncio.start_unix_server(hub.handle_control, path=path)
    os.chmod(path, 0o660)
    return server, path


async def main():
    loop = asyncio.get_running_loop()
    for d in (config.games_dir(), config.rom_dir()):
        try:
            os.makedirs(d, exist_ok=True)
        except OSError as e:
            log.warning("Cannot create %s: %s", d, e)
    hub = Hub(loop)
    control, sock_path = await open_control_socket(hub, config.control_socket())
    log.info("PixelPlus games started (API %s, control socket %s, ROMs in %s)",
             hub.api.base, sock_path, config.rom_dir())
    hub.events = EventStream(hub.api.ws_url(), hub.on_event)
    hub.events.start()
    stopping = asyncio.Event()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stopping.set)
    ticker = asyncio.ensure_future(hub.tick())
    await stopping.wait()
    log.info("Shutting down")
    ticker.cancel()
    await hub.events.stop()
    control.close()
    if hub.server:
        await hub.server.stop()
    # let a running game/arcade hand the matrix and the show back to pixelplusd
    hub.stop_event.set()
    hub.arcade_stop.set()
    for t in (hub.game_thread, hub.arcade_thread):
        if t is not None:
            await loop.run_in_executor(None, t.join, 5)
    if hub.busy_reason and hub.cfg.matrix:
        # an invite or test pattern was on the matrix: don't leave it frozen there
        await loop.run_in_executor(None, hub.api.overlay_enable, hub.cfg.matrix.prop_id, False)
    try:
        os.unlink(sock_path)
    except OSError:
        pass


def run():
    setup_logging()
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
    return 0
