"""Game audio out through ALSA's aplay.

The device is the show's own (``settings.audio.device``), so the game plays
through the same speakers / FM transmitter as the show; the show is paused
while a game runs.  Use a device that can be shared (``default``, a ``dmix``
or PipeWire/Pulse device) if pixelplusd keeps the card open while paused.

Writes happen on a thread with a short queue so a stalled sound card can
never slow the emulator down; if the queue fills, the oldest audio is dropped.
"""

import logging
import queue
import subprocess
import threading

import numpy as np

log = logging.getLogger("pixelplus_games.audio")


class AudioOut:
    def __init__(self, sample_rate, device="default", volume=80):
        self.sample_rate = sample_rate
        self.device = device or "default"
        self.gain = max(0, min(100, volume)) / 100.0
        self._q = queue.Queue(maxsize=12)  # ~200ms at 60 chunks/s
        self._proc = None
        self._thread = None
        self._err_thread = None
        self._err_tail = b""

    def start(self):
        cmd = ["aplay", "-q", "-D", self.device, "-t", "raw", "-f", "S16_LE", "-c", "2",
               "-r", str(self.sample_rate), "--buffer-time=100000", "-"]
        try:
            self._proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
                                          stderr=subprocess.PIPE)
        except OSError as e:
            log.warning("Could not start aplay, the game will be silent: %s", e)
            self._proc = None
            return
        self._thread = threading.Thread(target=self._writer, name="audio", daemon=True)
        self._thread.start()
        # aplay reports every underrun on stderr, even with -q. Nobody reading the pipe would let
        # it fill up during a long arcade session and freeze aplay (and so the game's sound).
        self._err_thread = threading.Thread(target=self._drain_stderr, args=(self._proc,),
                                            name="audio-err", daemon=True)
        self._err_thread.start()

    def _drain_stderr(self, proc):
        try:
            for line in iter(proc.stderr.readline, b""):
                self._err_tail = (self._err_tail + line)[-2048:]
        except (OSError, ValueError):
            pass

    def _last_error(self):
        lines = [l for l in self._err_tail.decode(errors="replace").splitlines() if l.strip()]
        return lines[-1].strip() if lines else ""

    def _writer(self):
        proc = self._proc
        while True:
            chunk = self._q.get()
            if chunk is None:
                break
            try:
                proc.stdin.write(chunk)
                proc.stdin.flush()  # a pipe buffer would otherwise hold ~3 frames of sound back
            except (BrokenPipeError, ValueError, OSError):
                try:
                    proc.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    pass
                if self._err_thread is not None:
                    self._err_thread.join(timeout=1)
                err = self._last_error()
                log.warning("aplay stopped%s", (": " + err) if err else "")
                break

    def play(self, pcm):
        if not pcm or self._proc is None:
            return
        if self.gain != 1.0:
            s = np.frombuffer(pcm, "<i2").astype(np.float32) * self.gain
            pcm = np.clip(s, -32768, 32767).astype("<i2").tobytes()
        try:
            self._q.put_nowait(pcm)
        except queue.Full:
            try:
                self._q.get_nowait()
                self._q.put_nowait(pcm)
            except (queue.Empty, queue.Full):
                pass

    def stop(self):
        if self._proc is None:
            return
        # drop pending audio so the game stops on time, then close
        while True:
            try:
                self._q.get_nowait()
            except queue.Empty:
                break
        self._q.put(None)
        if self._thread:
            self._thread.join(timeout=2)
            if self._thread.is_alive():
                # aplay stopped reading (a stuck sound card): closing stdin would wait for the
                # blocked write forever, so end aplay first, which fails that write.
                self._proc.kill()
                self._thread.join(timeout=2)
        try:
            self._proc.stdin.close()
        except (OSError, ValueError):
            pass
        try:
            self._proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self._proc.kill()
            self._proc.wait()
        if self._err_thread is not None:
            self._err_thread.join(timeout=2)
            self._err_thread = None
        if self._proc.stderr:
            self._proc.stderr.close()
        self._proc = None
