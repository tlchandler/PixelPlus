"""``python3 -m pixelplus_games``: run the games sidecar."""

import sys

from .server import run

if __name__ == "__main__":
    sys.exit(run())
