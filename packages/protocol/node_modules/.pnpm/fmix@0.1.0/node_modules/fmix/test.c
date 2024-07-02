#include <stdint.h>
#include <stdbool.h>

uint32_t fmix32 (uint32_t h) {
  h ^= h >> 16;
  h *= 0x85ebca6b;
  h ^= h >> 13;
  h *= 0xc2b2ae35;
  h ^= h >> 16;

  return h;
}

bool test (uint32_t input, uint32_t expected) {
  return (fmix32(input) == expected);
}

int main () {
  if (!test(0, 0)) return 1;

  if (!test(0x20645650, 0x8d69a732)) return 1;
  if (!test(0x35742f07, 0x9cd4a747)) return 1;
  if (!test(0x3a7d2a06, 0xf317e399)) return 1;
  if (!test(0x5456a322, 0x468db38a)) return 1;
  if (!test(0x6f5e7ee3, 0x84b3daac)) return 1;
  if (!test(0xafacfcf9, 0x1e273f97)) return 1;
  if (!test(0xb1bb772d, 0x19f54a85)) return 1;
  if (!test(0xb2f541ba, 0x22a57a9c)) return 1;
  if (!test(0xc4829a56, 0xcffc790b)) return 1;
  if (!test(0xdb12e192, 0x173ea289)) return 1;
  if (!test(0xdcaf6ceb, 0x4249660f)) return 1;
  if (!test(0xdeadbeef, 0x0de5c6a9)) return 1;
  if (!test(0xdeed8655, 0xa8180872)) return 1;
  if (!test(0xe458ddfc, 0xee1fc9fc)) return 1;
  if (!test(0xfd7e6623, 0xbde895f1)) return 1;
  if (!test(0xfe15c4d2, 0x67809cf5)) return 1;
  if (!test(0xff0ac0a8, 0x4074c32d)) return 1;

  return 0;
}
