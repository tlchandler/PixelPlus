"""Shared test setup: import paths, quiet logging, small helpers."""

import logging
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
GAMES = os.path.dirname(HERE)
for p in (GAMES, HERE):
    if p not in sys.path:
        sys.path.insert(0, p)

# The code under test logs warnings on purpose (unreachable daemon, HTTP fallback...).
# PIXELPLUS_TEST_LOG=1 shows them.
if not os.environ.get("PIXELPLUS_TEST_LOG"):
    logging.disable(logging.CRITICAL)


def wait_for(cond, timeout=3.0, step=0.01):
    """Poll ``cond`` until it is truthy or ``timeout`` passes; return its last value."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        v = cond()
        if v:
            return v
        time.sleep(step)
    return cond()
