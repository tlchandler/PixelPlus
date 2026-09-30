// Board pin defaults. Inputs and their pins come from the leader after
// adoption (Settings → Sensors); these are only the defaults a fresh node
// announces, and the I²C pins used for an INA219/INA226 current sensor.
#pragma once

#if defined(PP_BOARD_C3)
#define PP_HW "esp32c3"
#define PP_PIN_BOOT 9      // BOOT button (hold 10 s: factory reset)
#define PP_PIN_LED 8       // on-board WS2812 on the DevKitM-1
#define PP_LED_RGB 1
#define PP_PIN_SDA 1
#define PP_PIN_SCL 3
#define PP_DEFAULT_PIR 4
#define PP_DEFAULT_BTN 5
#define PP_DEFAULT_BEAM 7
#elif defined(PP_BOARD_S3)
#define PP_HW "esp32s3"
#define PP_PIN_BOOT 0
#define PP_PIN_LED 48      // WS2812 (v1.0 boards; v1.1 uses 38)
#define PP_LED_RGB 1
#define PP_PIN_SDA 8
#define PP_PIN_SCL 9
#define PP_DEFAULT_PIR 4
#define PP_DEFAULT_BTN 5
#define PP_DEFAULT_BEAM 6
#else  // classic ESP32 DevKit
#define PP_HW "esp32"
#define PP_PIN_BOOT 0
#define PP_PIN_LED 2
#define PP_LED_RGB 0
#define PP_PIN_SDA 21
#define PP_PIN_SCL 22
#define PP_DEFAULT_PIR 27
#define PP_DEFAULT_BTN 26
#define PP_DEFAULT_BEAM 25
#endif

#ifndef PP_FW_VERSION
#define PP_FW_VERSION "0.1.0"
#endif
