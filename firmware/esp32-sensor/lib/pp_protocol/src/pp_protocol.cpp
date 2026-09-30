#include "pp_protocol.h"

#include <stdio.h>
#include <stdlib.h>

#include "pp_sha256.h"

namespace pp {

std::string hex(const uint8_t* data, size_t len) {
  static const char* d = "0123456789abcdef";
  std::string s;
  s.reserve(len * 2);
  for (size_t i = 0; i < len; i++) {
    s.push_back(d[data[i] >> 4]);
    s.push_back(d[data[i] & 15]);
  }
  return s;
}

static int nib(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;
}

bool unhex(const std::string& s, uint8_t* out, size_t len) {
  if (s.size() != len * 2) return false;
  for (size_t i = 0; i < len; i++) {
    int a = nib(s[2 * i]), b = nib(s[2 * i + 1]);
    if (a < 0 || b < 0) return false;
    out[i] = (uint8_t)(a << 4 | b);
  }
  return true;
}

std::string sha256Hex(const std::string& data) {
  uint8_t d[32];
  sha256((const uint8_t*)data.data(), data.size(), d);
  return hex(d, 32);
}

std::string hmacHex(const std::string& key, const std::string& msg) {
  uint8_t d[32];
  hmacSha256((const uint8_t*)key.data(), key.size(), (const uint8_t*)msg.data(), msg.size(), d);
  return hex(d, 32);
}

bool ctEqual(const std::string& a, const std::string& b) {
  if (a.size() != b.size()) return false;
  uint8_t acc = 0;
  for (size_t i = 0; i < a.size(); i++) acc |= (uint8_t)(a[i] ^ b[i]);
  return acc == 0;
}

std::string sensorIdFromMac(const uint8_t mac[6]) { return "sn" + hex(mac + 2, 4); }

std::string deriveKey(const uint8_t shared[32], const std::string& leaderId,
                      const std::string& sensorId, const std::string& leaderPubHex,
                      const std::string& sensorPubHex) {
  std::string info = "pixelplus-sensor-key-v1\n" + leaderId + "\n" + sensorId + "\n" +
                     leaderPubHex + "\n" + sensorPubHex;
  uint8_t d[32];
  hmacSha256(shared, 32, (const uint8_t*)info.data(), info.size(), d);
  return hex(d, 32);
}

std::string adoptProof(const std::string& key, const std::string& leaderId,
                       const std::string& sensorId) {
  return hmacHex(key, "pixelplus-sensor-adopted-v1\n" + leaderId + "\n" + sensorId);
}

std::string jsonString(const std::string& s) {
  std::string o = "\"";
  for (char c : s) {
    switch (c) {
      case '"':
        o += "\\\"";
        break;
      case '\\':
        o += "\\\\";
        break;
      case '\n':
        o += "\\n";
        break;
      case '\r':
        o += "\\r";
        break;
      case '\t':
        o += "\\t";
        break;
      default:
        if ((unsigned char)c < 0x20) {
          char buf[8];
          snprintf(buf, sizeof buf, "\\u%04x", (unsigned)c);
          o += buf;
        } else {
          o.push_back(c);
        }
    }
  }
  o.push_back('"');
  return o;
}

std::string seal(const std::string& object, const std::string& key, const std::string& boot,
                 uint64_t seq) {
  std::string body = object;
  while (!body.empty() && (body.back() == ' ' || body.back() == '\n' || body.back() == '\r'))
    body.pop_back();
  if (!body.empty()) body.pop_back();  // '}'
  char num[24];
  snprintf(num, sizeof num, "%llu", (unsigned long long)seq);
  body += ",\"bt\":" + jsonString(boot) + ",\"sq\":" + num + "}";
  std::string mac = hmacHex(key, body);
  body.pop_back();
  body += ",\"mac\":\"" + mac + "\"}";
  return body;
}

bool verify(const std::string& packet, const std::string& key) {
  const size_t suffix = 8 + 64 + 2;  // ,"mac":" + hex + "}
  if (key.empty() || packet.size() < suffix + 2) return false;
  size_t split = packet.size() - suffix;
  if (packet.compare(split, 8, ",\"mac\":\"") != 0) return false;
  if (packet.compare(packet.size() - 2, 2, "\"}") != 0) return false;
  std::string mac = packet.substr(split + 8, 64);
  for (char& c : mac)
    if (c >= 'A' && c <= 'F') c = c - 'A' + 'a';
  std::string body = packet.substr(0, split) + "}";
  return ctEqual(hmacHex(key, body), mac);
}

static std::string upper(std::string s) {
  for (char& c : s)
    if (c >= 'a' && c <= 'z') c = c - 'a' + 'A';
  return s;
}

static std::string requestMac(const std::string& key, const std::string& method,
                              const std::string& path, const std::string& sender, int64_t ts,
                              const std::string& nonce, const std::string& body) {
  char tsb[24];
  snprintf(tsb, sizeof tsb, "%lld", (long long)ts);
  return hmacHex(key, "pixelplus-req-v1\n" + upper(method) + "\n" + path + "\n" + sender + "\n" +
                          tsb + "\n" + nonce + "\n" + sha256Hex(body));
}

std::string signRequest(const std::string& key, const std::string& sender, const std::string& method,
                        const std::string& path, int64_t ts, const std::string& nonce,
                        const std::string& body) {
  char tsb[24];
  snprintf(tsb, sizeof tsb, "%lld", (long long)ts);
  return "v1 " + sender + " " + tsb + " " + nonce + " " +
         requestMac(key, method, path, sender, ts, nonce, body);
}

bool verifyRequest(const std::string& header, const std::string& key, const std::string& method,
                   const std::string& path, const std::string& body, std::string* sender,
                   int64_t* ts, std::string* nonce) {
  // "v1 <sender> <ts> <nonce> <mac>"
  std::string parts[5];
  size_t n = 0, i = 0;
  while (i < header.size() && n < 6) {
    while (i < header.size() && header[i] == ' ') i++;
    if (i >= header.size()) break;
    size_t j = header.find(' ', i);
    if (j == std::string::npos) j = header.size();
    if (n == 5) return false;
    parts[n++] = header.substr(i, j - i);
    i = j;
  }
  if (n != 5 || parts[0] != "v1") return false;
  if (parts[1].empty() || parts[1].size() > 64 || parts[3].empty() || parts[3].size() > 64)
    return false;
  char* end = nullptr;
  long long t = strtoll(parts[2].c_str(), &end, 10);
  if (end == parts[2].c_str() || *end) return false;
  std::string mac = parts[4];
  for (char& c : mac)
    if (c >= 'A' && c <= 'F') c = c - 'A' + 'a';
  if (!ctEqual(requestMac(key, method, path, parts[1], t, parts[3], body), mac)) return false;
  if (sender) *sender = parts[1];
  if (ts) *ts = t;
  if (nonce) *nonce = parts[3];
  return true;
}

std::string replyMac(const std::string& key, const std::string& nonce, const std::string& what) {
  return hmacHex(key, "pixelplus-reply-v1\n" + nonce + "\n" + what);
}

bool verifyTimeProof(const std::string& key, const std::string& header, const std::string& nonce,
                     int64_t* now) {
  size_t sp = header.find(' ');
  if (sp == std::string::npos) return false;
  std::string n = header.substr(0, sp);
  std::string mac = header.substr(sp + 1);
  while (!mac.empty() && (mac.back() == ' ' || mac.back() == '\r' || mac.back() == '\n'))
    mac.pop_back();
  char* end = nullptr;
  long long t = strtoll(n.c_str(), &end, 10);
  if (end == n.c_str() || *end) return false;
  if (!ctEqual(hmacHex(key, "pixelplus-time-v1\n" + n + "\n" + nonce), mac)) return false;
  if (now) *now = t;
  return true;
}

}  // namespace pp
