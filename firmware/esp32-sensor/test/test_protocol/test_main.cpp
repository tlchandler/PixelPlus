// Protocol test vectors shared with the Rust leader
// (crates/pixelplus-daemon/src/services/sensornodes.rs tests read the same
// test/vectors.json). Run: pio test -e native
#include <ArduinoJson.h>
#include <unity.h>

#include <fstream>
#include <sstream>
#include <string>

#include "pp_protocol.h"
#include "pp_sha256.h"
#include "pp_x25519.h"

static JsonDocument doc;

static std::string S(JsonVariantConst v) { return v.as<std::string>(); }

static void load() {
  std::ifstream f("test/vectors.json");
  std::stringstream ss;
  ss << f.rdbuf();
  DeserializationError err = deserializeJson(doc, ss.str());
  TEST_ASSERT_TRUE_MESSAGE(!err, "test/vectors.json must be readable (run from the project folder)");
}

void test_sha256_known_answers() {
  // FIPS 180-2 "abc".
  TEST_ASSERT_EQUAL_STRING("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                           pp::sha256Hex("abc").c_str());
  // RFC 4231 test case 2.
  TEST_ASSERT_EQUAL_STRING("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
                           pp::hmacHex("Jefe", "what do ya want for nothing?").c_str());
  // A key longer than a block (RFC 4231 test case 6).
  std::string k(131, '\xaa');
  TEST_ASSERT_EQUAL_STRING("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
                           pp::hmacHex(k, "Test Using Larger Than Block-Size Key - Hash Key First").c_str());
}

void test_x25519_rfc7748() {
  JsonObjectConst v = doc["x25519"];
  uint8_t a[32], b[32], apub[32], bpub[32], s1[32], s2[32];
  TEST_ASSERT_TRUE(pp::unhex(S(v["leaderPrivate"]), a, 32));
  TEST_ASSERT_TRUE(pp::unhex(S(v["sensorPrivate"]), b, 32));
  pp::x25519Base(apub, a);
  pp::x25519Base(bpub, b);
  TEST_ASSERT_EQUAL_STRING(S(v["leaderPublic"]).c_str(), pp::hex(apub, 32).c_str());
  TEST_ASSERT_EQUAL_STRING(S(v["sensorPublic"]).c_str(), pp::hex(bpub, 32).c_str());
  pp::x25519(s1, a, bpub);
  pp::x25519(s2, b, apub);
  TEST_ASSERT_EQUAL_STRING(S(v["shared"]).c_str(), pp::hex(s1, 32).c_str());
  TEST_ASSERT_EQUAL_MEMORY(s1, s2, 32);
}

void test_key_derivation_and_proof() {
  JsonObjectConst kd = doc["keyDerivation"];
  uint8_t shared[32];
  TEST_ASSERT_TRUE(pp::unhex(S(kd["shared"]), shared, 32));
  std::string key = pp::deriveKey(shared, S(kd["leaderId"]), S(kd["sensorId"]),
                                  S(kd["leaderPublic"]), S(kd["sensorPublic"]));
  TEST_ASSERT_EQUAL_STRING(S(kd["key"]).c_str(), key.c_str());
  JsonObjectConst ap = doc["adoptProof"];
  TEST_ASSERT_EQUAL_STRING(
      S(ap["proof"]).c_str(),
      pp::adoptProof(S(ap["key"]), S(ap["leaderId"]), S(ap["sensorId"])).c_str());
}

void test_datagrams() {
  for (JsonObjectConst d : doc["datagrams"].as<JsonArrayConst>()) {
    std::string p = pp::seal(S(d["json"]), S(d["key"]), S(d["boot"]), d["seq"].as<uint64_t>());
    TEST_ASSERT_EQUAL_STRING(S(d["packet"]).c_str(), p.c_str());
    TEST_ASSERT_TRUE(pp::verify(p, S(d["key"])));
    std::string bad = p;
    bad[10] ^= 1;
    TEST_ASSERT_FALSE(pp::verify(bad, S(d["key"])));
    TEST_ASSERT_FALSE(pp::verify(p, std::string(64, '0')));
  }
}

void test_signed_requests_and_replies() {
  for (JsonObjectConst r : doc["requests"].as<JsonArrayConst>()) {
    std::string h = pp::signRequest(S(r["key"]), S(r["sender"]), S(r["method"]), S(r["path"]),
                                    r["ts"].as<int64_t>(), S(r["nonce"]), S(r["body"]));
    TEST_ASSERT_EQUAL_STRING(S(r["header"]).c_str(), h.c_str());
    std::string sender, nonce;
    int64_t ts = 0;
    TEST_ASSERT_TRUE(pp::verifyRequest(h, S(r["key"]), S(r["method"]), S(r["path"]), S(r["body"]),
                                       &sender, &ts, &nonce));
    TEST_ASSERT_EQUAL_STRING(S(r["sender"]).c_str(), sender.c_str());
    TEST_ASSERT_FALSE(
        pp::verifyRequest(h, S(r["key"]), S(r["method"]), "/elsewhere", S(r["body"]), 0, 0, 0));
  }
  JsonObjectConst rp = doc["reply"];
  TEST_ASSERT_EQUAL_STRING(S(rp["what"]).c_str(), pp::sha256Hex(S(rp["body"])).c_str());
  TEST_ASSERT_EQUAL_STRING(S(rp["mac"]).c_str(),
                           pp::replyMac(S(rp["key"]), S(rp["nonce"]), S(rp["what"])).c_str());
  JsonObjectConst tp = doc["timeProof"];
  int64_t now = 0;
  TEST_ASSERT_TRUE(pp::verifyTimeProof(S(tp["key"]), S(tp["header"]), S(tp["nonce"]), &now));
  TEST_ASSERT_EQUAL_INT64(tp["now"].as<int64_t>(), now);
  TEST_ASSERT_FALSE(pp::verifyTimeProof(S(tp["key"]), S(tp["header"]), "othernonce", &now));
}

void test_sensor_ids() {
  for (JsonObjectConst m : doc["sensorIds"].as<JsonArrayConst>()) {
    std::string s = S(m["mac"]);
    uint8_t mac[6];
    unsigned v[6];
    TEST_ASSERT_EQUAL(6, sscanf(s.c_str(), "%x:%x:%x:%x:%x:%x", &v[0], &v[1], &v[2], &v[3], &v[4], &v[5]));
    for (int i = 0; i < 6; i++) mac[i] = (uint8_t)v[i];
    TEST_ASSERT_EQUAL_STRING(S(m["id"]).c_str(), pp::sensorIdFromMac(mac).c_str());
  }
}

void setUp(void) {}
void tearDown(void) {}

int main(int, char**) {
  UNITY_BEGIN();
  load();
  RUN_TEST(test_sha256_known_answers);
  RUN_TEST(test_x25519_rfc7748);
  RUN_TEST(test_key_derivation_and_proof);
  RUN_TEST(test_datagrams);
  RUN_TEST(test_signed_requests_and_replies);
  RUN_TEST(test_sensor_ids);
  return UNITY_END();
}
