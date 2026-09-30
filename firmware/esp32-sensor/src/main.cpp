// PixelPlus ESP32 sensor node (F20): motion sensors, buttons, beam breaks,
// contacts and an optional INA219/INA226 current sensor, reported to the
// PixelPlus show leader over authenticated UDP (docs/ARCHITECTURE.md §7.5,
// §12.16; firmware/esp32-sensor/README.md).
//
// States: no Wi-Fi → captive-portal hotspot "PixelPlus-Sensor-XXXX";
// on Wi-Fi, unadopted → beacons every 2 s until a leader adopts it (key
// exchange over HTTP); adopted → events + heartbeats to the leader, config
// fetched from the leader (signed both ways).

#include <Arduino.h>
#include <ArduinoJson.h>
#include <DNSServer.h>
#include <HTTPClient.h>
#include <Preferences.h>
#include <WebServer.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <Wire.h>
#include <esp_system.h>
#if __has_include(<esp_random.h>)
#include <esp_random.h>
#endif

#include <string>
#include <vector>

#include "board.h"
#include "pp_debounce.h"
#include "pp_protocol.h"
#include "pp_x25519.h"

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Input {
  std::string id;
  std::string kind;  // motion | button | beam | contact | current
  uint8_t pin = 0;   // GPIO, or the I²C address for "current"
  bool activeLow = false;
  uint32_t debounceMs = 30;
  uint32_t holdMs = 0;
  float shuntMilliohms = 100.0f;
  pp::Debouncer deb;
  // current sensor
  bool ina226 = false;
  bool present = false;
  float amps = 0, volts = 0;
};

struct PendingEvent {
  std::string input;
  int state;
  uint32_t ms;
  uint64_t seq;
  uint8_t tries;
  uint32_t nextAt;
};

static Preferences prefs;
static WebServer http(80);
static DNSServer dns;
static WiFiUDP udp;

static std::string nodeId;   // "sn" + MAC tail
static std::string bootId;   // random per boot
static std::string name;     // friendly name
static uint64_t seq = 0;

// Adoption.
static std::string leaderId, leaderKey, leaderUrl;
static IPAddress leaderIp;
static uint16_t leaderHttpPort = 80, sensorPort = pp::kSensorPort;
static std::string leaderBoot;       // `lb` from the last sack
static int64_t timeOffset = 0;       // leader unix time - millis()/1000
static bool timeKnown = false;
static int64_t lastAuthTs = 0;       // newest accepted signed leader request
static uint64_t lastCmdSeq = 0;
static std::string lastCmdBoot;
static uint32_t adoptWindowUntil = 0;  // BOOT short press opens re-adoption

// Configuration from the leader.
static std::string cfgVersion;
static uint32_t statusEvery = 10;
static bool cfgFetchWanted = false;
static uint32_t cfgFetchAfter = 0;
static std::vector<Input> inputs;

static std::vector<PendingEvent> pending;
static bool portal = false;
static uint32_t identifyUntil = 0;
static uint32_t lastBeacon = 0, lastStatus = 0, lastInaRead = 0, lastWifiTry = 0;
static uint32_t eventsSent = 0, eventsLost = 0;

static bool adopted() { return !leaderKey.empty() && !leaderId.empty(); }

static int64_t leaderNow() { return (int64_t)(millis() / 1000) + timeOffset; }

// ---------------------------------------------------------------------------
// LED
// ---------------------------------------------------------------------------

static void led(uint8_t r, uint8_t g, uint8_t b) {
#if PP_LED_RGB
#if defined(ESP_ARDUINO_VERSION_MAJOR) && ESP_ARDUINO_VERSION_MAJOR >= 3
  rgbLedWrite(PP_PIN_LED, r / 8, g / 8, b / 8);
#else
  neopixelWrite(PP_PIN_LED, r / 8, g / 8, b / 8);
#endif
#else
  digitalWrite(PP_PIN_LED, (r | g | b) ? HIGH : LOW);
#endif
}

// Blink codes: identify = fast white; portal = slow blue; unadopted = amber
// pulse every 2 s; adopted = short green blink on each event.
static void ledTick() {
  uint32_t t = millis();
  if (t < identifyUntil) {
    (t / 120) % 2 ? led(255, 255, 255) : led(0, 0, 0);
  } else if (portal) {
    (t / 1000) % 2 ? led(0, 0, 255) : led(0, 0, 0);
  } else if (t < adoptWindowUntil) {
    (t / 250) % 2 ? led(255, 0, 255) : led(0, 0, 0);
  } else if (!adopted()) {
    (t % 2000) < 100 ? led(255, 120, 0) : led(0, 0, 0);
  } else {
    led(0, 0, 0);
  }
}

// ---------------------------------------------------------------------------
// Persistence (NVS)
// ---------------------------------------------------------------------------

static std::string getStr(const char* ns, const char* key) {
  prefs.begin(ns, true);
  String v = prefs.getString(key, "");
  prefs.end();
  return std::string(v.c_str());
}

static void putStr(const char* ns, const char* key, const std::string& v) {
  prefs.begin(ns, false);
  prefs.putString(key, v.c_str());
  prefs.end();
}

static void saveAdoption() {
  prefs.begin("adopt", false);
  prefs.putString("leader", leaderId.c_str());
  prefs.putString("key", leaderKey.c_str());
  prefs.putString("url", leaderUrl.c_str());
  prefs.putUShort("sport", sensorPort);
  prefs.putLong64("lts", lastAuthTs);
  prefs.end();
}

static void clearAdoption() {
  leaderId.clear();
  leaderKey.clear();
  leaderUrl.clear();
  leaderBoot.clear();
  cfgVersion.clear();
  pending.clear();
  prefs.begin("adopt", false);
  prefs.clear();
  prefs.end();
  prefs.begin("cfg", false);
  prefs.clear();
  prefs.end();
}

static void factoryReset() {
  Serial.println("Factory reset");
  for (const char* ns : {"adopt", "cfg", "wifi", "node"}) {
    prefs.begin(ns, false);
    prefs.clear();
    prefs.end();
  }
  delay(200);
  ESP.restart();
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

static uint16_t inaRead(uint8_t addr, uint8_t reg, bool* ok) {
  Wire.beginTransmission(addr);
  Wire.write(reg);
  if (Wire.endTransmission(false) != 0 || Wire.requestFrom((int)addr, 2) != 2) {
    *ok = false;
    return 0;
  }
  uint16_t hi = Wire.read(), lo = Wire.read();
  *ok = true;
  return (uint16_t)(hi << 8 | lo);
}

static void inaWrite(uint8_t addr, uint8_t reg, uint16_t v) {
  Wire.beginTransmission(addr);
  Wire.write(reg);
  Wire.write((uint8_t)(v >> 8));
  Wire.write((uint8_t)v);
  Wire.endTransmission();
}

static void inaProbe(Input& in) {
  Wire.beginTransmission(in.pin);
  in.present = Wire.endTransmission() == 0;
  if (!in.present) return;
  bool ok = false;
  uint16_t mfr = inaRead(in.pin, 0xFE, &ok);
  in.ina226 = ok && mfr == 0x5449;  // "TI"; the INA219 has no ID register
  if (in.ina226) {
    inaWrite(in.pin, 0x00, 0x4527);  // 16-sample average, 1.1 ms conversions, continuous
  } else {
    inaWrite(in.pin, 0x00, 0x399F);  // INA219 default: 32 V, ±320 mV, 12-bit
  }
}

static void inaMeasure(Input& in) {
  if (!in.present) {
    inaProbe(in);
    if (!in.present) return;
  }
  bool ok1 = false, ok2 = false;
  int16_t shunt = (int16_t)inaRead(in.pin, 0x01, &ok1);
  uint16_t bus = inaRead(in.pin, 0x02, &ok2);
  if (!ok1 || !ok2) {
    in.present = false;
    return;
  }
  float microVolts = in.ina226 ? shunt * 2.5f : shunt * 10.0f;
  float volts = in.ina226 ? bus * 1.25e-3f : (bus >> 3) * 4e-3f;
  float mohm = in.shuntMilliohms > 0.01f ? in.shuntMilliohms : 100.0f;
  // I = V / R = (µV · 1e-6) / (mΩ · 1e-3)
  float amps = microVolts / mohm * 1e-3f;
  // Light smoothing (readings every second, reported every 10 s).
  in.amps = in.amps == 0 ? amps : in.amps * 0.7f + amps * 0.3f;
  in.volts = volts;
}

static void applyInputs() {
  uint32_t now = millis();
  bool wire = false;
  for (auto& in : inputs) {
    if (in.kind == "current") {
      if (!wire) {
        Wire.begin(PP_PIN_SDA, PP_PIN_SCL);
        wire = true;
      }
      inaProbe(in);
      continue;
    }
    // PIRs drive their output (pull-down keeps a loose wire quiet); buttons,
    // contacts and open-collector beam receivers need a pull-up.
    pinMode(in.pin, in.kind == "motion" ? INPUT_PULLDOWN : INPUT_PULLUP);
    in.deb.configure(in.activeLow, in.debounceMs, in.holdMs);
    in.deb.reset(digitalRead(in.pin), now);
  }
}

static void defaultInputs() {
  inputs.clear();
  Input pir;
  pir.id = "pir1";
  pir.kind = "motion";
  pir.pin = PP_DEFAULT_PIR;
  inputs.push_back(pir);
  Input btn;
  btn.id = "btn1";
  btn.kind = "button";
  btn.pin = PP_DEFAULT_BTN;
  btn.activeLow = true;
  inputs.push_back(btn);
}

// Parse the leader's `/cluster/sensor-config/<id>` answer.
static bool loadConfigJson(const std::string& body) {
  JsonDocument doc;
  if (deserializeJson(doc, body)) return false;
  JsonArrayConst arr = doc["inputs"].as<JsonArrayConst>();
  if (arr.isNull()) return false;
  std::vector<Input> next;
  for (JsonObjectConst o : arr) {
    Input in;
    in.id = o["id"] | "";
    in.kind = o["kind"] | "button";
    in.pin = o["pin"] | 0;
    in.activeLow = o["activeLow"] | false;
    in.debounceMs = o["debounceMs"] | 30;
    in.holdMs = o["holdMs"] | 0;
    in.shuntMilliohms = o["shuntMilliohms"] | 100.0f;
    if (in.id.empty() || in.id.size() > 16) continue;
    if (in.kind != "current" && in.pin > 48) continue;
    next.push_back(in);
    if (next.size() >= 8) break;
  }
  inputs = next;
  cfgVersion = doc["version"] | "";
  statusEvery = doc["statusEverySec"] | 10;
  if (statusEvery < 2 || statusEvery > 300) statusEvery = 10;
  std::string n = doc["name"] | "";
  if (!n.empty()) name = n;
  applyInputs();
  return true;
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

static void sendTo(const IPAddress& ip, uint16_t port, const std::string& data) {
  udp.beginPacket(ip, port);
  udp.write((const uint8_t*)data.data(), data.size());
  udp.endPacket();
}

static std::string inputIdsJson() {
  std::string s = "[";
  for (size_t i = 0; i < inputs.size(); i++) {
    if (i) s += ",";
    s += pp::jsonString(inputs[i].id);
  }
  return s + "]";
}

static void sendBeacon() {
  std::string m = "{\"t\":\"sbeacon\",\"id\":" + pp::jsonString(nodeId) +
                  ",\"name\":" + pp::jsonString(name) + ",\"hw\":\"" PP_HW "\",\"ver\":\"" PP_FW_VERSION
                  "\",\"http\":80,\"adoptedBy\":" +
                  (adopted() ? pp::jsonString(leaderId) : std::string("null")) +
                  ",\"inputs\":" + inputIdsJson() + ",\"proto\":" + std::to_string(pp::kProto) + "}";
  sendTo(IPAddress(255, 255, 255, 255), sensorPort, m);
}

static std::string fmtFloat(float v, int digits) {
  char b[24];
  snprintf(b, sizeof b, "%.*f", digits, (double)v);
  return b;
}

static void sendStatus() {
  if (!adopted()) return;
  std::string ins = "{", amps = "{", volts = "{";
  bool fi = true, fa = true;
  for (auto& in : inputs) {
    if (in.kind == "current") {
      if (!in.present) continue;
      amps += (fa ? "" : ",") + pp::jsonString(in.id) + ":" + fmtFloat(in.amps, 3);
      volts += (fa ? "" : ",") + pp::jsonString(in.id) + ":" + fmtFloat(in.volts, 2);
      fa = false;
    } else {
      ins += (fi ? "" : ",") + pp::jsonString(in.id) + ":" + std::to_string(in.deb.state());
      fi = false;
    }
  }
  ins += "}";
  amps += "}";
  volts += "}";
  std::string m = "{\"t\":\"sstatus\",\"id\":" + pp::jsonString(nodeId) +
                  ",\"rssi\":" + std::to_string(WiFi.RSSI()) +
                  ",\"uptime\":" + std::to_string(millis() / 1000) + ",\"ver\":\"" PP_FW_VERSION
                  "\",\"cfg\":" + pp::jsonString(cfgVersion) + ",\"inputs\":" + ins +
                  ",\"amps\":" + amps + ",\"volts\":" + volts + "}";
  sendTo(leaderIp, sensorPort, pp::seal(m, leaderKey, bootId, ++seq));
}

static void transmit(PendingEvent& e) {
  std::string m = "{\"t\":\"sevent\",\"id\":" + pp::jsonString(nodeId) +
                  ",\"input\":" + pp::jsonString(e.input) + ",\"state\":" + std::to_string(e.state) +
                  ",\"ms\":" + std::to_string(e.ms) + ",\"lb\":" + pp::jsonString(leaderBoot) + "}";
  sendTo(leaderIp, sensorPort, pp::seal(m, leaderKey, bootId, e.seq));
}

static void queueEvent(const std::string& input, int state) {
  if (!adopted()) return;
  if (pending.size() >= 16) {
    pending.erase(pending.begin());
    eventsLost++;
  }
  PendingEvent e{input, state, millis(), ++seq, 0, 0};
  transmit(e);
  e.tries = 1;
  e.nextAt = millis() + 100;
  pending.push_back(e);
  eventsSent++;
}

// Retries at 100, 200 and 400 ms.
static void retryEvents() {
  uint32_t now = millis();
  for (size_t i = 0; i < pending.size();) {
    PendingEvent& e = pending[i];
    if ((int32_t)(now - e.nextAt) >= 0) {
      if (e.tries >= 4) {
        eventsLost++;
        pending.erase(pending.begin() + i);
        continue;
      }
      transmit(e);
      e.nextAt = now + (100u << e.tries);
      e.tries++;
    }
    i++;
  }
}

static void handleSack(JsonDocument& doc) {
  std::string sb = doc["sb"] | "";
  if (sb != bootId || (doc["id"] | "") != nodeId) return;  // not an answer to this run
  uint64_t ack = doc["ack"].as<uint64_t>();
  bool ok = doc["ok"] | false;
  std::string lb = doc["lb"] | "";
  int64_t now = doc["now"].as<int64_t>();
  if (now > 1600000000) {
    timeOffset = now - (int64_t)(millis() / 1000);
    timeKnown = true;
  }
  if (!lb.empty() && lb != leaderBoot) leaderBoot = lb;
  std::string cfg = doc["cfg"] | "";
  if (!cfg.empty() && cfg != cfgVersion && !cfgFetchWanted) {
    cfgFetchWanted = true;
    cfgFetchAfter = millis();
  }
  for (size_t i = 0; i < pending.size(); i++) {
    if (pending[i].seq != ack) continue;
    if (ok) {
      pending.erase(pending.begin() + i);
    } else {
      // The leader restarted (new boot id): resend at once with the new one.
      PendingEvent e = pending[i];
      pending.erase(pending.begin() + i);
      e.seq = ++seq;
      e.tries = 1;
      e.nextAt = millis() + 100;
      transmit(e);
      pending.push_back(e);
    }
    break;
  }
}

static void handleCmd(JsonDocument& doc) {
  if ((doc["id"] | "") != nodeId) return;
  std::string bt = doc["bt"] | "";
  uint64_t sq = doc["sq"].as<uint64_t>();
  // Replay: the leader's current boot and a growing sequence number.
  if (bt.empty() || (bt == lastCmdBoot && sq <= lastCmdSeq)) return;
  if (!leaderBoot.empty() && bt != leaderBoot) return;
  lastCmdBoot = bt;
  lastCmdSeq = sq;
  std::string cmd = doc["cmd"] | "";
  if (cmd == "identify") identifyUntil = millis() + 8000;
}

static void pollUdp() {
  int n = udp.parsePacket();
  if (n <= 0) return;
  if (n > 2048) {
    udp.flush();
    return;
  }
  std::string buf(n, '\0');
  udp.read((uint8_t*)&buf[0], n);
  if (!adopted() || !(udp.remoteIP() == leaderIp)) return;
  if (!pp::verify(buf, leaderKey)) return;
  JsonDocument doc;
  if (deserializeJson(doc, buf)) return;
  std::string t = doc["t"] | "";
  if (t == "sack") handleSack(doc);
  if (t == "scmd") handleCmd(doc);
}

// ---------------------------------------------------------------------------
// Leader HTTP (configuration)
// ---------------------------------------------------------------------------

static std::string randomHex(size_t bytes) {
  std::vector<uint8_t> b(bytes);
  esp_fill_random(b.data(), b.size());
  return pp::hex(b.data(), b.size());
}

// GET <leader>/api/v1/cluster/sensor-config/<id>, signed with our key; the
// answer must carry X-PixelPlus-Reply for our nonce. A 401 with our clock
// off comes with the leader's time (MACed), and we retry once.
static bool fetchConfig() {
  std::string path = "/api/v1/cluster/sensor-config/" + nodeId;
  for (int attempt = 0; attempt < 2; attempt++) {
    std::string nonce = randomHex(16);
    int64_t ts = timeKnown ? leaderNow() : 0;
    std::string auth = pp::signRequest(leaderKey, nodeId, "GET", path, ts, nonce, "");
    HTTPClient c;
    c.setTimeout(5000);
    std::string url = "http://" + std::string(leaderIp.toString().c_str()) + ":" +
                      std::to_string(leaderHttpPort) + path;
    if (!c.begin(url.c_str())) return false;
    c.addHeader("X-PixelPlus-Auth", auth.c_str());
    c.addHeader("X-PixelPlus-Request", "1");
    const char* keep[] = {"X-PixelPlus-Reply", "X-PixelPlus-Time"};
    c.collectHeaders(keep, 2);
    int code = c.GET();
    if (code == 200) {
      std::string body = c.getString().c_str();
      std::string reply = c.header("X-PixelPlus-Reply").c_str();
      c.end();
      if (!pp::ctEqual(reply, pp::replyMac(leaderKey, nonce, pp::sha256Hex(body)))) return false;
      if (!loadConfigJson(body)) return false;
      putStr("cfg", "json", body);
      Serial.printf("Config %s: %u inputs\n", cfgVersion.c_str(), (unsigned)inputs.size());
      return true;
    }
    std::string hint = c.header("X-PixelPlus-Time").c_str();
    c.end();
    int64_t now = 0;
    if (code == 401 && !hint.empty() && pp::verifyTimeProof(leaderKey, hint, nonce, &now)) {
      timeOffset = now - (int64_t)(millis() / 1000);
      timeKnown = true;
      continue;
    }
    Serial.printf("Config fetch failed: HTTP %d\n", code);
    return false;
  }
  return false;
}

// ---------------------------------------------------------------------------
// Local HTTP: adoption, release, info, Wi-Fi setup
// ---------------------------------------------------------------------------

static bool parseLeaderUrl(const std::string& url) {
  // http://<ipv4>:<port>
  if (url.rfind("http://", 0) != 0) return false;
  std::string rest = url.substr(7);
  size_t colon = rest.find(':');
  std::string host = rest.substr(0, colon);
  uint16_t port = 80;
  if (colon != std::string::npos) port = (uint16_t)atoi(rest.substr(colon + 1).c_str());
  IPAddress ip;
  if (!ip.fromString(host.c_str()) || port == 0) return false;
  leaderIp = ip;
  leaderHttpPort = port;
  return true;
}

static void sendJson(int code, const std::string& body) {
  http.send(code, "application/json", body.c_str());
}

static void handleInfo() {
  std::string m = "{\"id\":" + pp::jsonString(nodeId) + ",\"name\":" + pp::jsonString(name) +
                  ",\"hw\":\"" PP_HW "\",\"ver\":\"" PP_FW_VERSION "\",\"adoptedBy\":" +
                  (adopted() ? pp::jsonString(leaderId) : std::string("null")) +
                  ",\"inputs\":" + inputIdsJson() + ",\"proto\":" + std::to_string(pp::kProto) +
                  ",\"eventsSent\":" + std::to_string(eventsSent) +
                  ",\"eventsLost\":" + std::to_string(eventsLost) + "}";
  sendJson(200, m);
}

// Check a request signed by our leader (re-adoption, release).
static bool leaderSigned(const std::string& method, const std::string& path, const std::string& body) {
  if (!adopted() || !http.hasHeader("X-PixelPlus-Auth")) return false;
  std::string sender, nonce;
  int64_t ts = 0;
  if (!pp::verifyRequest(http.header("X-PixelPlus-Auth").c_str(), leaderKey, method, path, body,
                         &sender, &ts, &nonce))
    return false;
  if (sender != leaderId || ts <= lastAuthTs) return false;  // monotonic: no replays
  if (timeKnown && llabs(ts - leaderNow()) > 300) return false;
  lastAuthTs = ts;
  return true;
}

static void handleAdopt() {
  std::string body = http.arg("plain").c_str();
  JsonDocument doc;
  if (deserializeJson(doc, body)) return sendJson(400, "{\"error\":\"bad json\"}");
  std::string lid = doc["leaderId"] | "";
  std::string url = doc["leaderUrl"] | "";
  std::string dh = doc["dh"] | "";
  uint16_t sport = doc["sensorPort"] | pp::kSensorPort;
  uint8_t peer[32];
  if (lid.empty() || lid.size() > 32 || !pp::unhex(dh, peer, 32))
    return sendJson(400, "{\"error\":\"bad request\"}");
  // Trust on first use; afterwards only the current leader (signed) or
  // within 10 minutes of a short BOOT press.
  bool windowOpen = (int32_t)(adoptWindowUntil - millis()) > 0;
  if (adopted() && !windowOpen && !leaderSigned("POST", "/adopt", body))
    return sendJson(409, "{\"error\":\"adopted by another show\"}");
  std::string oldUrl = leaderUrl;
  if (!parseLeaderUrl(url)) {
    parseLeaderUrl(oldUrl);
    return sendJson(400, "{\"error\":\"bad leaderUrl\"}");
  }
  uint8_t priv[32], pub[32], shared[32];
  esp_fill_random(priv, 32);
  pp::x25519Base(pub, priv);
  pp::x25519(shared, priv, peer);
  std::string sensorPub = pp::hex(pub, 32);
  std::string key = pp::deriveKey(shared, lid, nodeId, dh, sensorPub);
  memset(priv, 0, sizeof priv);
  memset(shared, 0, sizeof shared);
  leaderId = lid;
  leaderKey = key;
  leaderUrl = url;
  sensorPort = sport;
  leaderBoot.clear();
  lastAuthTs = 0;
  cfgVersion.clear();
  adoptWindowUntil = 0;
  saveAdoption();
  cfgFetchWanted = true;
  cfgFetchAfter = millis() + 1500;
  std::string ins = "[";
  for (size_t i = 0; i < inputs.size(); i++) {
    const Input& in = inputs[i];
    if (i) ins += ",";
    ins += "{\"id\":" + pp::jsonString(in.id) + ",\"pin\":" + std::to_string(in.pin) +
           ",\"kind\":" + pp::jsonString(in.kind) + ",\"activeLow\":" + (in.activeLow ? "true" : "false") + "}";
  }
  ins += "]";
  sendJson(200, "{\"id\":" + pp::jsonString(nodeId) + ",\"dh\":\"" + sensorPub + "\",\"proof\":\"" +
                    pp::adoptProof(key, lid, nodeId) + "\",\"hw\":\"" PP_HW "\",\"ver\":\"" PP_FW_VERSION
                    "\",\"inputs\":" + ins + "}");
  Serial.printf("Adopted by %s (%s)\n", leaderId.c_str(), leaderUrl.c_str());
  lastStatus = 0;  // say hello right away
}

static void handleRelease() {
  std::string body = http.arg("plain").c_str();
  if (!leaderSigned("POST", "/release", body)) return sendJson(401, "{\"error\":\"not signed by the leader\"}");
  sendJson(200, "{\"ok\":true}");
  Serial.println("Released by the leader");
  clearAdoption();
  defaultInputs();
  applyInputs();
}

static const char* PAGE_HEAD =
    "<!doctype html><html><head><meta charset=utf-8><meta name=viewport "
    "content='width=device-width,initial-scale=1'><title>PixelPlus sensor</title><style>"
    "body{font-family:system-ui,sans-serif;background:#0f1115;color:#e8e8ea;margin:0;padding:24px}"
    ".card{max-width:420px;margin:auto;background:#1a1d24;border-radius:14px;padding:20px}"
    "h1{font-size:20px;margin:0 0 4px}p{color:#a9adb8}label{display:block;margin:12px 0 4px}"
    "input,select{width:100%;box-sizing:border-box;padding:10px;border-radius:8px;border:1px solid #333;"
    "background:#0f1115;color:#e8e8ea;font-size:16px}button{margin-top:16px;width:100%;padding:12px;"
    "border:0;border-radius:8px;background:#d33a2c;color:#fff;font-size:16px;font-weight:600}"
    "code{background:#0f1115;padding:2px 6px;border-radius:6px}</style></head><body><div class=card>";

static std::string htmlEscape(const std::string& s) {
  std::string o;
  for (char c : s) {
    if (c == '<') o += "&lt;";
    else if (c == '>') o += "&gt;";
    else if (c == '&') o += "&amp;";
    else if (c == '"') o += "&quot;";
    else if (c == '\'') o += "&#39;";
    else o.push_back(c);
  }
  return o;
}

static void handlePortalPage() {
  std::string page = PAGE_HEAD;
  page += "<h1>PixelPlus sensor</h1><p>Connect this sensor to the Wi-Fi your show runs on.</p>";
  page += "<form method=post action=/wifi><label>Wi-Fi network</label><select name=ssid>";
  int n = WiFi.scanComplete();
  if (n == WIFI_SCAN_FAILED || n == WIFI_SCAN_RUNNING) n = 0;
  for (int i = 0; i < n && i < 20; i++) {
    std::string s = WiFi.SSID(i).c_str();
    if (s.empty()) continue;
    page += "<option>" + htmlEscape(s) + "</option>";
  }
  page += "</select><label>…or type its name</label><input name=ssid2 autocomplete=off>";
  page += "<label>Wi-Fi password</label><input name=pass type=password>";
  page += "<label>Name for this sensor</label><input name=name maxlength=40 value=\"" + htmlEscape(name) + "\">";
  page += "<button>Save and connect</button></form><p>ID <code>" + nodeId + "</code> · v" PP_FW_VERSION "</p></div></body></html>";
  http.send(200, "text/html", page.c_str());
  if (WiFi.scanComplete() != WIFI_SCAN_RUNNING) WiFi.scanNetworks(true);
}

static void handleStatusPage() {
  std::string page = PAGE_HEAD;
  page += "<h1>" + htmlEscape(name) + "</h1><p>PixelPlus sensor <code>" + nodeId + "</code> · v" PP_FW_VERSION "</p>";
  page += adopted() ? "<p>Connected to show leader <code>" + htmlEscape(leaderId) + "</code>.</p>"
                    : "<p>Waiting to be added: open PixelPlus → Settings → Sensors.</p>";
  page += "<p>Inputs: ";
  for (auto& in : inputs) {
    page += "<code>" + htmlEscape(in.id) + "=";
    page += in.kind == "current" ? fmtFloat(in.amps, 2) + " A" : std::to_string(in.deb.state());
    page += "</code> ";
  }
  page += "</p><p>Wi-Fi " + std::to_string(WiFi.RSSI()) + " dBm. Hold BOOT 10 s to reset.</p></div></body></html>";
  http.send(200, "text/html", page.c_str());
}

static void handleWifiSave() {
  // Only from the setup hotspot, or while unadopted / the adopt window is open.
  bool allowed = portal || !adopted() || (int32_t)(adoptWindowUntil - millis()) > 0;
  if (!allowed) return http.send(403, "text/plain", "Press BOOT briefly first.");
  std::string ssid = http.arg("ssid2").c_str();
  if (ssid.empty()) ssid = http.arg("ssid").c_str();
  std::string pass = http.arg("pass").c_str();
  std::string nm = http.arg("name").c_str();
  if (ssid.empty() || ssid.size() > 32 || pass.size() > 64)
    return http.send(400, "text/plain", "Choose a network.");
  putStr("wifi", "ssid", ssid);
  putStr("wifi", "pass", pass);
  if (!nm.empty() && nm.size() <= 40) putStr("node", "name", nm);
  std::string page = PAGE_HEAD;
  page += "<h1>Saved</h1><p>The sensor restarts and joins <b>" + htmlEscape(ssid) +
          "</b>. Then add it in PixelPlus → Settings → Sensors.</p></div></body></html>";
  http.send(200, "text/html", page.c_str());
  delay(500);
  ESP.restart();
}

static void handleNotFound() {
  if (portal) {
    // Captive portal: send every unknown URL to the setup page.
    http.sendHeader("Location", "http://192.168.4.1/", true);
    http.send(302, "text/plain", "");
  } else {
    http.send(404, "text/plain", "Not found");
  }
}

static void startHttp() {
  const char* keep[] = {"X-PixelPlus-Auth"};
  http.collectHeaders(keep, 1);
  http.on("/", HTTP_GET, [] { portal ? handlePortalPage() : handleStatusPage(); });
  http.on("/info", HTTP_GET, handleInfo);
  http.on("/adopt", HTTP_POST, [] {
    if (portal) return sendJson(503, "{\"error\":\"not on the show network\"}");
    handleAdopt();
  });
  http.on("/release", HTTP_POST, handleRelease);
  http.on("/wifi", HTTP_POST, handleWifiSave);
  // OS captive-portal probes.
  for (const char* p : {"/generate_204", "/hotspot-detect.html", "/ncsi.txt", "/connecttest.txt"}) {
    http.on(p, HTTP_GET, [] { portal ? handlePortalPage() : http.send(204); });
  }
  http.onNotFound(handleNotFound);
  http.begin();
}

// ---------------------------------------------------------------------------
// Wi-Fi
// ---------------------------------------------------------------------------

static void startPortal() {
  portal = true;
  std::string tail = nodeId.substr(nodeId.size() - 4);
  for (auto& c : tail) c = (char)toupper((unsigned char)c);
  std::string ap = "PixelPlus-Sensor-" + tail;
  WiFi.mode(WIFI_AP_STA);
  WiFi.softAPConfig(IPAddress(192, 168, 4, 1), IPAddress(192, 168, 4, 1), IPAddress(255, 255, 255, 0));
  WiFi.softAP(ap.c_str());
  dns.start(53, "*", IPAddress(192, 168, 4, 1));
  WiFi.scanNetworks(true);
  Serial.printf("Setup hotspot \"%s\" (http://192.168.4.1)\n", ap.c_str());
}

static bool connectWifi(uint32_t timeoutMs) {
  std::string ssid = getStr("wifi", "ssid");
  if (ssid.empty()) return false;
  std::string pass = getStr("wifi", "pass");
  WiFi.mode(portal ? WIFI_AP_STA : WIFI_STA);
  WiFi.setSleep(false);  // lower latency for events
  WiFi.setHostname(("pixelplus-" + nodeId).c_str());
  WiFi.begin(ssid.c_str(), pass.c_str());
  uint32_t start = millis();
  while (WiFi.status() != WL_CONNECTED && millis() - start < timeoutMs) {
    delay(100);
    ledTick();
  }
  return WiFi.status() == WL_CONNECTED;
}

// ---------------------------------------------------------------------------
// BOOT button: short press = allow re-adoption for 10 min; 10 s = reset
// ---------------------------------------------------------------------------

static void pollBootButton() {
  static uint32_t downSince = 0;
  bool down = digitalRead(PP_PIN_BOOT) == LOW;
  uint32_t now = millis();
  if (down && !downSince) downSince = now;
  if (down && downSince && now - downSince > 10000) factoryReset();
  if (!down && downSince) {
    uint32_t held = now - downSince;
    downSince = 0;
    if (held > 50 && held < 2000) {
      adoptWindowUntil = now + 10 * 60 * 1000;
      Serial.println("Adoption allowed for 10 minutes");
    }
  }
}

// ---------------------------------------------------------------------------
// Arduino entry points
// ---------------------------------------------------------------------------

void setup() {
  Serial.begin(115200);
  delay(50);
  pinMode(PP_PIN_BOOT, INPUT_PULLUP);
#if !PP_LED_RGB
  pinMode(PP_PIN_LED, OUTPUT);
#endif
  uint8_t mac[6];
  WiFi.macAddress(mac);
  nodeId = pp::sensorIdFromMac(mac);
  bootId = randomHex(4);
  name = getStr("node", "name");
  if (name.empty()) {
    std::string tail = nodeId.substr(nodeId.size() - 4);
    for (auto& c : tail) c = (char)toupper((unsigned char)c);
    name = "PixelPlus-Sensor-" + tail;
  }
  leaderId = getStr("adopt", "leader");
  leaderKey = getStr("adopt", "key");
  leaderUrl = getStr("adopt", "url");
  prefs.begin("adopt", true);
  sensorPort = prefs.getUShort("sport", pp::kSensorPort);
  lastAuthTs = prefs.getLong64("lts", 0);
  prefs.end();
  if (adopted() && !parseLeaderUrl(leaderUrl)) clearAdoption();
  std::string cfg = getStr("cfg", "json");
  if (cfg.empty() || !loadConfigJson(cfg)) {
    defaultInputs();
    applyInputs();
  }
  Serial.printf("\nPixelPlus sensor %s v%s (%s), boot %s\n", nodeId.c_str(), PP_FW_VERSION, PP_HW,
                bootId.c_str());
  if (!connectWifi(30000)) startPortal();
  startHttp();
  udp.begin(pp::kSensorPort);
  if (adopted()) {
    cfgFetchWanted = true;
    cfgFetchAfter = millis() + 2000;
  }
}

void loop() {
  uint32_t now = millis();
  http.handleClient();
  if (portal) dns.processNextRequest();
  pollBootButton();
  ledTick();

  // Leave the hotspot once the saved Wi-Fi works again; retry every minute.
  if (portal && now - lastWifiTry > 60000) {
    lastWifiTry = now;
    if (connectWifi(8000)) {
      dns.stop();
      WiFi.softAPdisconnect(true);
      WiFi.mode(WIFI_STA);
      portal = false;
      Serial.printf("Connected: %s\n", WiFi.localIP().toString().c_str());
    }
  }
  if (!portal && WiFi.status() != WL_CONNECTED) {
    static uint32_t lostSince = 0;
    if (!lostSince) lostSince = now;
    if (now - lostSince > 120000) {
      lostSince = 0;
      startPortal();  // Wi-Fi gone for 2 minutes: offer setup (keeps retrying)
    }
    delay(10);
    return;
  }

  // Inputs (every 5 ms) and the current sensor (every second).
  static uint32_t lastPoll = 0;
  if (now - lastPoll >= 5) {
    lastPoll = now;
    for (auto& in : inputs) {
      if (in.kind == "current") continue;
      if (in.deb.update(digitalRead(in.pin), now)) {
        Serial.printf("%s → %d\n", in.id.c_str(), in.deb.state());
        queueEvent(in.id, in.deb.state());
        if (in.deb.state() && adopted()) identifyUntil = max(identifyUntil, now + 150);
      }
    }
  }
  if (now - lastInaRead >= 1000) {
    lastInaRead = now;
    for (auto& in : inputs)
      if (in.kind == "current") inaMeasure(in);
  }

  pollUdp();
  retryEvents();

  uint32_t beaconEvery = adopted() ? 10000 : 2000;
  if (!portal && now - lastBeacon >= beaconEvery) {
    lastBeacon = now;
    sendBeacon();
  }
  if (adopted() && (lastStatus == 0 || now - lastStatus >= statusEvery * 1000)) {
    lastStatus = now;
    sendStatus();
  }
  if (adopted() && cfgFetchWanted && (int32_t)(now - cfgFetchAfter) >= 0) {
    cfgFetchWanted = false;
    if (!fetchConfig()) {
      cfgFetchWanted = true;
      cfgFetchAfter = now + 30000;
    }
  }
  delay(1);
}
