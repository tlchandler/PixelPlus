// X25519 (RFC 7748) Diffie-Hellman, constant time. Adapted from the public
// domain TweetNaCl `crypto_scalarmult` (Bernstein et al.), so the sensor
// node needs no particular mbedTLS version. ~60 ms on an ESP32-C3; used only
// at adoption.
#pragma once
#include <stdint.h>

namespace pp {

// out = X25519(scalar, point). All 32 bytes, little endian as in RFC 7748.
void x25519(uint8_t out[32], const uint8_t scalar[32], const uint8_t point[32]);
// out = X25519(scalar, 9): the public key of a private scalar.
void x25519Base(uint8_t out[32], const uint8_t scalar[32]);

}  // namespace pp
