#!/usr/bin/env python3
"""Regenerate the minisign test fixture (a TEST key, never used for releases).

Writes minisign/test.pub, minisign/pixelplus_9.9.9_arm64.deb (dummy bytes) and its
pre-hashed ("ED", BLAKE2b-512) signature exactly as `minisign -S` does, so the daemon's
verifier (crates/pixelplus-daemon/src/services/updates.rs) is checked against the
reference format. Pure Python (RFC 8032 reference Ed25519), no dependencies.
"""
import base64
import hashlib
import os

# --- RFC 8032 Ed25519 (reference implementation, slow but dependency-free) ---
_p = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_d = -121665 * pow(121666, _p - 2, _p) % _p
_I = pow(2, (_p - 1) // 4, _p)


def _inv(x):
    return pow(x, _p - 2, _p)


def _xrecover(y):
    xx = (y * y - 1) * _inv(_d * y * y + 1)
    x = pow(xx, (_p + 3) // 8, _p)
    if (x * x - xx) % _p != 0:
        x = (x * _I) % _p
    if x % 2 != 0:
        x = _p - x
    return x


_By = 4 * _inv(5) % _p
_B = (_xrecover(_By), _By, 1, _xrecover(_By) * _By % _p)


def _add(P, Q):
    A = (P[1] - P[0]) * (Q[1] - Q[0]) % _p
    B = (P[1] + P[0]) * (Q[1] + Q[0]) % _p
    C = 2 * P[3] * Q[3] * _d % _p
    D = 2 * P[2] * Q[2] % _p
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E * F % _p, G * H % _p, F * G % _p, E * H % _p)


def _mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            Q = _add(Q, P)
        P = _add(P, P)
        s >>= 1
    return Q


def _encode(P):
    zi = _inv(P[2])
    x, y = P[0] * zi % _p, P[1] * zi % _p
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _h(m):
    return hashlib.sha512(m).digest()


def _secret(seed):
    h = _h(seed)
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def ed25519_public(seed):
    a, _ = _secret(seed)
    return _encode(_mul(a, _B))


def ed25519_sign(seed, msg):
    a, prefix = _secret(seed)
    A = _encode(_mul(a, _B))
    r = int.from_bytes(_h(prefix + msg), "little") % _L
    R = _encode(_mul(r, _B))
    k = int.from_bytes(_h(R + A + msg), "little") % _L
    return R + int.to_bytes((r + k * a) % _L, 32, "little")

HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "minisign")
SEED = bytes(range(32))  # deterministic test key
KEY_ID = bytes.fromhex("0123456789abcdef")


def main():
    os.makedirs(HERE, exist_ok=True)
    pk = ed25519_public(SEED)
    b64 = lambda b: base64.b64encode(b).decode()
    with open(os.path.join(HERE, "test.pub"), "w") as f:
        f.write(f"untrusted comment: minisign public key {KEY_ID[::-1].hex().upper()} (TEST ONLY)\n")
        f.write(b64(b"Ed" + KEY_ID + pk) + "\n")
    data = b"!<arch>\nnot really a deb, just test bytes for the signature fixture\n" * 50
    name = "pixelplus_9.9.9_arm64.deb"
    with open(os.path.join(HERE, name), "wb") as f:
        f.write(data)
    sig = ed25519_sign(SEED, hashlib.blake2b(data, digest_size=64).digest())
    trusted = f"timestamp:1790000000\tfile:{name}\thashed"
    gsig = ed25519_sign(SEED, sig + trusted.encode())
    with open(os.path.join(HERE, name + ".minisig"), "w") as f:
        f.write("untrusted comment: signature from minisign secret key (TEST ONLY)\n")
        f.write(b64(b"ED" + KEY_ID + sig) + "\n")
        f.write(f"trusted comment: {trusted}\n")
        f.write(b64(gsig) + "\n")


if __name__ == "__main__":
    main()
