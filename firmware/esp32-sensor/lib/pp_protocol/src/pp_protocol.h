// PixelPlus sensor-node wire protocol (docs/ARCHITECTURE.md §7.5, §12.16).
// Everything here is pure and shared with the leader through
// test/vectors.json (the Rust tests read the same file).
#pragma once
#include <stddef.h>
#include <stdint.h>

#include <string>

namespace pp {

constexpr int kProto = 1;
constexpr uint16_t kSensorPort = 32422;

std::string hex(const uint8_t* data, size_t len);
// Decode exactly `len` bytes of hex; false on bad input.
bool unhex(const std::string& s, uint8_t* out, size_t len);
std::string sha256Hex(const std::string& data);
// HMAC-SHA256 keyed with the ASCII bytes of `key` (keys travel as 64 hex chars).
std::string hmacHex(const std::string& key, const std::string& msg);
// Constant-time string compare.
bool ctEqual(const std::string& a, const std::string& b);

// Node id from the Wi-Fi MAC: "sn" + last 4 bytes in lowercase hex.
std::string sensorIdFromMac(const uint8_t mac[6]);

// key = hex(HMAC-SHA256(shared, "pixelplus-sensor-key-v1\n<leader>\n<sensor>\n<leaderPub>\n<sensorPub>"))
std::string deriveKey(const uint8_t shared[32], const std::string& leaderId,
                      const std::string& sensorId, const std::string& leaderPubHex,
                      const std::string& sensorPubHex);
// proof = hex(HMAC(key, "pixelplus-sensor-adopted-v1\n<leader>\n<sensor>"))
std::string adoptProof(const std::string& key, const std::string& leaderId,
                       const std::string& sensorId);

// Datagram canonicalization: `object` is a serialized JSON object; appends
// ,"bt":"<boot>","sq":<seq> and ,"mac":"<hmac of everything before + '}'>"}.
std::string seal(const std::string& object, const std::string& key, const std::string& boot,
                 uint64_t seq);
// The packet carries a valid MAC for `key`.
bool verify(const std::string& packet, const std::string& key);

// X-PixelPlus-Auth value for a request:
// "v1 <sender> <ts> <nonce> hex(HMAC(key, "pixelplus-req-v1\n<METHOD>\n<path>\n<sender>\n<ts>\n<nonce>\n<sha256hex(body)>"))"
std::string signRequest(const std::string& key, const std::string& sender, const std::string& method,
                        const std::string& path, int64_t ts, const std::string& nonce,
                        const std::string& body);
// Parse and check an X-PixelPlus-Auth header (MAC only; time and nonce are
// the caller's job). Fills sender / ts / nonce on success.
bool verifyRequest(const std::string& header, const std::string& key, const std::string& method,
                   const std::string& path, const std::string& body, std::string* sender,
                   int64_t* ts, std::string* nonce);
// X-PixelPlus-Reply MAC: hex(HMAC(key, "pixelplus-reply-v1\n<nonce>\n<what>")).
std::string replyMac(const std::string& key, const std::string& nonce, const std::string& what);
// X-PixelPlus-Time "<now> <hex(HMAC(key, "pixelplus-time-v1\n<now>\n<nonce>"))>".
bool verifyTimeProof(const std::string& key, const std::string& header, const std::string& nonce,
                     int64_t* now);

// JSON string literal (with quotes) for building messages by hand.
std::string jsonString(const std::string& s);

}  // namespace pp
