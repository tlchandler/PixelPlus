"""Send a command to the running fpp-mariobros daemon.

    python3 -m mario.ctl status
    python3 -m mario.ctl invite [flashes] [text|qr|both]
    python3 -m mario.ctl stop
    python3 -m mario.ctl test
"""

import json
import socket
import sys

from . import config


def send(req, timeout=5):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(config.CONTROL_SOCKET)
        s.sendall((json.dumps(req) + "\n").encode())
        data = b""
        while not data.endswith(b"\n"):
            chunk = s.recv(4096)
            if not chunk:
                break
            data += chunk
        return json.loads(data or b"{}")
    except OSError as e:
        return {"ok": False, "error": "fpp-mariobros daemon not reachable: %s" % e}
    finally:
        s.close()


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    req = {"cmd": argv[0]}
    if argv[0] == "invite":
        if len(argv) > 1 and argv[1].isdigit() and int(argv[1]) > 0:
            req["flashes"] = int(argv[1])
        if len(argv) > 2 and argv[2] in ("text", "qr", "both"):
            req["style"] = argv[2]
    reply = send(req)
    print(json.dumps(reply))
    return 0 if reply.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
