// Input debouncing / hold logic. Run: pio test -e native
#include <unity.h>

#include "pp_debounce.h"

void test_debounce_ignores_bounces() {
  pp::Debouncer d;
  d.configure(false, 30, 0);
  TEST_ASSERT_FALSE(d.update(0, 0));
  // Bounce: high for 10 ms, low again.
  TEST_ASSERT_FALSE(d.update(1, 100));
  TEST_ASSERT_FALSE(d.update(1, 110));
  TEST_ASSERT_FALSE(d.update(0, 111));
  TEST_ASSERT_FALSE(d.update(0, 200));
  TEST_ASSERT_EQUAL(0, d.state());
  // A real press: stable for 30 ms.
  TEST_ASSERT_FALSE(d.update(1, 300));
  TEST_ASSERT_FALSE(d.update(1, 329));
  TEST_ASSERT_TRUE(d.update(1, 330));
  TEST_ASSERT_EQUAL(1, d.state());
  TEST_ASSERT_FALSE(d.update(1, 400));
  TEST_ASSERT_FALSE(d.update(0, 500));
  TEST_ASSERT_TRUE(d.update(0, 530));
  TEST_ASSERT_EQUAL(0, d.state());
}

void test_active_low_button() {
  pp::Debouncer d;
  d.configure(true, 20, 0);
  d.reset(1, 0);  // pulled up: idle
  TEST_ASSERT_EQUAL(0, d.state());
  d.update(0, 10);
  TEST_ASSERT_TRUE(d.update(0, 30));
  TEST_ASSERT_EQUAL(1, d.state());
}

void test_hold_merges_retriggers() {
  pp::Debouncer d;
  d.configure(false, 0, 5000);
  d.reset(0, 0);
  TEST_ASSERT_TRUE(d.update(1, 1000));
  TEST_ASSERT_FALSE(d.update(0, 1200));  // PIR drops quickly
  TEST_ASSERT_FALSE(d.update(1, 3000));  // re-trigger inside the hold: no new event
  TEST_ASSERT_FALSE(d.update(0, 3100));
  TEST_ASSERT_FALSE(d.update(0, 7999));  // hold counts from the last activity (3000)
  TEST_ASSERT_TRUE(d.update(0, 8000));
  TEST_ASSERT_EQUAL(0, d.state());
}

void test_millis_wraparound() {
  pp::Debouncer d;
  d.configure(false, 30, 0);
  d.reset(0, 0xFFFFFFF0u);
  d.update(1, 0xFFFFFFF8u);
  TEST_ASSERT_TRUE(d.update(1, 0x00000020u));  // 40 ms later across the wrap
}

void setUp(void) {}
void tearDown(void) {}

int main(int, char**) {
  UNITY_BEGIN();
  RUN_TEST(test_debounce_ignores_bounces);
  RUN_TEST(test_active_low_button);
  RUN_TEST(test_hold_merges_retriggers);
  RUN_TEST(test_millis_wraparound);
  return UNITY_END();
}
