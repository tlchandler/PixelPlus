// Input conditioning for sensor-node inputs: active-low inversion, debounce
// (a level must hold `debounceMs` before it counts) and hold (an activation
// is reported as active for at least `holdMs`, which merges PIR re-triggers
// and stretches short beam breaks). Pure logic, tested on the host.
#pragma once
#include <stdint.h>

namespace pp {

class Debouncer {
 public:
  void configure(bool activeLow, uint32_t debounceMs, uint32_t holdMs);
  // Feed the raw pin level at time `nowMs`. Returns true when the reported
  // (logical, 1 = active) state changed; read it with state().
  bool update(int rawLevel, uint32_t nowMs);
  int state() const { return reported_; }
  // Force a known state (after reconfiguration / at boot) without an event.
  void reset(int rawLevel, uint32_t nowMs);

 private:
  bool activeLow_ = false;
  uint32_t debounceMs_ = 30;
  uint32_t holdMs_ = 0;
  int stable_ = 0;     // debounced logical level
  int candidate_ = 0;  // level waiting to become stable
  uint32_t candidateSince_ = 0;
  int reported_ = 0;
  uint32_t activeSince_ = 0;
  bool init_ = false;
};

}  // namespace pp
