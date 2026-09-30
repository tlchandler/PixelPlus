// SHA-256 and HMAC-SHA256 (FIPS 180-4, RFC 2104). Portable, no dependencies,
// so the protocol code builds and is tested identically on the ESP32 and on a
// PC (PlatformIO "native" environment).
#pragma once
#include <stddef.h>
#include <stdint.h>

namespace pp {

class Sha256 {
 public:
  Sha256();
  void update(const uint8_t* data, size_t len);
  void finish(uint8_t out[32]);

 private:
  void block(const uint8_t* p);
  uint32_t h_[8];
  uint8_t buf_[64];
  size_t used_ = 0;
  uint64_t total_ = 0;
};

void sha256(const uint8_t* data, size_t len, uint8_t out[32]);
void hmacSha256(const uint8_t* key, size_t keyLen, const uint8_t* msg, size_t len, uint8_t out[32]);

}  // namespace pp
