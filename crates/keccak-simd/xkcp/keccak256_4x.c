/*
 * 4-way parallel Keccak-256 using XKCP KeccakP-1600-times4 (AVX2).
 *
 * Performs the full Keccak-256 sponge (absorb, pad, permute, squeeze)
 * for 4 independent messages of the same length simultaneously.
 *
 * SPDX-License-Identifier: CC0-1.0
 */

#include <stdint.h>
#include <string.h>
#include "KeccakP-1600-times4-SnP.h"

/*
 * Keccak-256 parameters:
 *   rate     = 1088 bits = 136 bytes = 17 lanes
 *   capacity = 512 bits
 *   output   = 256 bits = 32 bytes = 4 lanes
 *
 * For messages <= 135 bytes (which covers 20-byte addresses and 32-byte
 * storage keys), the entire message fits in one block. The sponge is:
 *   1. Initialize state to zero
 *   2. XOR message bytes into the state (absorb)
 *   3. Apply Keccak padding (0x01 after message, 0x80 at end of rate)
 *   4. Permute (24 rounds)
 *   5. Extract first 32 bytes (squeeze)
 */

void keccak256_4x(
    const uint8_t *in0, const uint8_t *in1,
    const uint8_t *in2, const uint8_t *in3,
    unsigned int inlen,
    uint8_t *out0, uint8_t *out1,
    uint8_t *out2, uint8_t *out3)
{
    KeccakP1600times4_states states;
    const unsigned int rate = 136; /* bytes */

    /* 1. Initialize all 4 states to zero */
    KeccakP1600times4_InitializeAll(&states);

    /* 2. Absorb: XOR input bytes into each instance's state */
    KeccakP1600times4_AddBytes(&states, 0, in0, 0, inlen);
    KeccakP1600times4_AddBytes(&states, 1, in1, 0, inlen);
    KeccakP1600times4_AddBytes(&states, 2, in2, 0, inlen);
    KeccakP1600times4_AddBytes(&states, 3, in3, 0, inlen);

    /* 3. Keccak padding: domain separator 0x01 after message,
     *    then 0x80 at position (rate - 1) */
    KeccakP1600times4_AddByte(&states, 0, 0x01, inlen);
    KeccakP1600times4_AddByte(&states, 1, 0x01, inlen);
    KeccakP1600times4_AddByte(&states, 2, 0x01, inlen);
    KeccakP1600times4_AddByte(&states, 3, 0x01, inlen);

    KeccakP1600times4_AddByte(&states, 0, 0x80, rate - 1);
    KeccakP1600times4_AddByte(&states, 1, 0x80, rate - 1);
    KeccakP1600times4_AddByte(&states, 2, 0x80, rate - 1);
    KeccakP1600times4_AddByte(&states, 3, 0x80, rate - 1);

    /* 4. Permute all 4 states (24 rounds of Keccak-f[1600]) */
    KeccakP1600times4_PermuteAll_24rounds(&states);

    /* 5. Squeeze: extract first 32 bytes from each instance */
    KeccakP1600times4_ExtractBytes(&states, 0, out0, 0, 32);
    KeccakP1600times4_ExtractBytes(&states, 1, out1, 0, 32);
    KeccakP1600times4_ExtractBytes(&states, 2, out2, 0, 32);
    KeccakP1600times4_ExtractBytes(&states, 3, out3, 0, 32);
}
