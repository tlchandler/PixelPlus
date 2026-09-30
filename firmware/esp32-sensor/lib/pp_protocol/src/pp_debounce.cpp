#include "pp_debounce.h"

namespace pp {

void Debouncer::configure(bool activeLow, uint32_t debounceMs, uint32_t holdMs) {
  activeLow_ = activeLow;
  debounceMs_ = debounceMs;
  holdMs_ = holdMs;
}

void Debouncer::reset(int rawLevel, uint32_t nowMs) {
  int logical = (rawLevel != 0) != activeLow_ ? 1 : 0;
  stable_ = candidate_ = reported_ = logical;
  candidateSince_ = activeSince_ = nowMs;
  init_ = true;
}

bool Debouncer::update(int rawLevel, uint32_t nowMs) {
  if (!init_) {
    reset(rawLevel, nowMs);
    return false;
  }
  int logical = (rawLevel != 0) != activeLow_ ? 1 : 0;
  if (logical != candidate_) {
    candidate_ = logical;
    candidateSince_ = nowMs;
  }
  if (candidate_ != stable_ && (uint32_t)(nowMs - candidateSince_) >= debounceMs_) {
    stable_ = candidate_;
  }
  int want = stable_;
  if (want == 1) {
    // Hold counts from the last moment the input was active.
    activeSince_ = nowMs;
    if (reported_ == 0) {
      reported_ = 1;
      return true;
    }
    return false;
  }
  if (reported_ == 1 && (uint32_t)(nowMs - activeSince_) >= holdMs_) {
    reported_ = 0;
    return true;
  }
  return false;
}

}  // namespace pp
