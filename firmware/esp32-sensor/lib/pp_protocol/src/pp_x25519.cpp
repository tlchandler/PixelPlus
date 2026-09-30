#include "pp_x25519.h"

namespace pp {

namespace {
typedef int64_t gf[16];
const gf k121665 = {0xDB41, 1};

void car25519(int64_t* o) {
  for (int i = 0; i < 16; i++) {
    o[i] += (int64_t)1 << 16;
    int64_t c = o[i] >> 16;
    o[(i + 1) * (i < 15)] += c - 1 + 37 * (c - 1) * (i == 15);
    o[i] -= c * 65536;
  }
}

void sel25519(int64_t* p, int64_t* q, int b) {
  int64_t c = ~((int64_t)b - 1);
  for (int i = 0; i < 16; i++) {
    int64_t t = c & (p[i] ^ q[i]);
    p[i] ^= t;
    q[i] ^= t;
  }
}

void pack25519(uint8_t* o, const int64_t* n) {
  gf m, t;
  for (int i = 0; i < 16; i++) t[i] = n[i];
  car25519(t);
  car25519(t);
  car25519(t);
  for (int j = 0; j < 2; j++) {
    m[0] = t[0] - 0xffed;
    for (int i = 1; i < 15; i++) {
      m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
      m[i - 1] &= 0xffff;
    }
    m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
    int b = (int)((m[15] >> 16) & 1);
    m[14] &= 0xffff;
    sel25519(t, m, 1 - b);
  }
  for (int i = 0; i < 16; i++) {
    o[2 * i] = (uint8_t)(t[i] & 0xff);
    o[2 * i + 1] = (uint8_t)(t[i] >> 8);
  }
}

void unpack25519(int64_t* o, const uint8_t* n) {
  for (int i = 0; i < 16; i++) o[i] = n[2 * i] + ((int64_t)n[2 * i + 1] << 8);
  o[15] &= 0x7fff;
}

void add(int64_t* o, const int64_t* a, const int64_t* b) {
  for (int i = 0; i < 16; i++) o[i] = a[i] + b[i];
}

void sub(int64_t* o, const int64_t* a, const int64_t* b) {
  for (int i = 0; i < 16; i++) o[i] = a[i] - b[i];
}

void mul(int64_t* o, const int64_t* a, const int64_t* b) {
  int64_t t[31];
  for (int i = 0; i < 31; i++) t[i] = 0;
  for (int i = 0; i < 16; i++)
    for (int j = 0; j < 16; j++) t[i + j] += a[i] * b[j];
  for (int i = 0; i < 15; i++) t[i] += 38 * t[i + 16];
  for (int i = 0; i < 16; i++) o[i] = t[i];
  car25519(o);
  car25519(o);
}

void sqr(int64_t* o, const int64_t* a) { mul(o, a, a); }

void inv25519(int64_t* o, const int64_t* in) {
  gf c;
  for (int a = 0; a < 16; a++) c[a] = in[a];
  for (int a = 253; a >= 0; a--) {
    sqr(c, c);
    if (a != 2 && a != 4) mul(c, c, in);
  }
  for (int a = 0; a < 16; a++) o[a] = c[a];
}
}  // namespace

void x25519(uint8_t out[32], const uint8_t scalar[32], const uint8_t point[32]) {
  uint8_t z[32];
  int64_t x[80];
  gf a, b, c, d, e, f;
  for (int i = 0; i < 31; i++) z[i] = scalar[i];
  z[31] = (scalar[31] & 127) | 64;
  z[0] &= 248;
  unpack25519(x, point);
  for (int i = 0; i < 16; i++) {
    b[i] = x[i];
    d[i] = a[i] = c[i] = 0;
  }
  a[0] = d[0] = 1;
  for (int i = 254; i >= 0; --i) {
    int r = (z[i >> 3] >> (i & 7)) & 1;
    sel25519(a, b, r);
    sel25519(c, d, r);
    add(e, a, c);
    sub(a, a, c);
    add(c, b, d);
    sub(b, b, d);
    sqr(d, e);
    sqr(f, a);
    mul(a, c, a);
    mul(c, b, e);
    add(e, a, c);
    sub(a, a, c);
    sqr(b, a);
    sub(c, d, f);
    mul(a, c, k121665);
    add(a, a, d);
    mul(c, c, a);
    mul(a, d, f);
    mul(d, b, x);
    sqr(b, e);
    sel25519(a, b, r);
    sel25519(c, d, r);
  }
  for (int i = 0; i < 16; i++) {
    x[i + 16] = a[i];
    x[i + 32] = c[i];
    x[i + 48] = b[i];
    x[i + 64] = d[i];
  }
  inv25519(x + 32, x + 32);
  mul(x + 16, x + 16, x + 32);
  pack25519(out, x + 16);
  // Wipe secrets from the stack.
  volatile uint8_t* vz = z;
  for (int i = 0; i < 32; i++) vz[i] = 0;
}

void x25519Base(uint8_t out[32], const uint8_t scalar[32]) {
  static const uint8_t nine[32] = {9};
  x25519(out, scalar, nine);
}

}  // namespace pp
