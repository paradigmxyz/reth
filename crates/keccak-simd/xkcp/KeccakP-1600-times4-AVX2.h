/*
The Keccak-p permutations, designed by Guido Bertoni, Joan Daemen, Michaël Peeters and Gilles Van Assche.

Implementation by Gilles Van Assche and Ronny Van Keer, hereby denoted as "the implementer".

For more information, feedback or questions, please refer to the Keccak Team website:
https://keccak.team/

To the extent possible under law, the implementer has waived all copyright
and related or neighboring rights to the source code in this file.
http://creativecommons.org/publicdomain/zero/1.0/

---

Please refer to PlSnP-documentation.h for more details.
*/

#ifndef _KeccakP_1600_times4_AVX2_h_
#define _KeccakP_1600_times4_AVX2_h_

#include <stdint.h>
#include <immintrin.h>
#include "config.h"
#include "PlSnP-common.h"

typedef __m256i V256;

typedef struct {
    V256 A[25];
} KeccakP1600times4_SIMD256_states;

#ifndef KeccakP1600times4_AVX2_implementation_config
    #define KeccakP1600times4_AVX2_implementation_config "default: 12 rounds unrolled"
    #define KeccakP1600times4_AVX2_unrolling 12
#endif

#define KeccakP1600times4_AVX2_GetImplementation() \
    ("256-bit SIMD implementation (" KeccakP1600times4_AVX2_implementation_config ")")
#define KeccakP1600times4_AVX2_GetFeatures() \
    (PlSnP_Feature_Main \
        | PlSnP_Feature_SpongeAbsorb \
        | PlSnP_Feature_Farfalle \
        | PlSnP_Feature_KangarooTwelve)

#define KeccakP1600times4_AVX2_StaticInitialize()
void KeccakP1600times4_AVX2_InitializeAll(KeccakP1600times4_SIMD256_states *states);
#define KeccakP1600times4_AVX2_AddByte(states, instanceIndex, byte, offset) \
    ((unsigned char*)(states))[(instanceIndex)*8 + ((offset)/8)*4*8 + (offset)%8] ^= (byte)
void KeccakP1600times4_AVX2_AddBytes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, const unsigned char *data, unsigned int offset, unsigned int length);
void KeccakP1600times4_AVX2_AddLanesAll(KeccakP1600times4_SIMD256_states *states, const unsigned char *data, unsigned int laneCount, unsigned int laneOffset);
void KeccakP1600times4_AVX2_OverwriteBytes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, const unsigned char *data, unsigned int offset, unsigned int length);
void KeccakP1600times4_AVX2_OverwriteLanesAll(KeccakP1600times4_SIMD256_states *states, const unsigned char *data, unsigned int laneCount, unsigned int laneOffset);
void KeccakP1600times4_AVX2_OverwriteWithZeroes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, unsigned int byteCount);
void KeccakP1600times4_AVX2_PermuteAll_4rounds(KeccakP1600times4_SIMD256_states *states);
void KeccakP1600times4_AVX2_PermuteAll_6rounds(KeccakP1600times4_SIMD256_states *states);
void KeccakP1600times4_AVX2_PermuteAll_12rounds(KeccakP1600times4_SIMD256_states *states);
void KeccakP1600times4_AVX2_PermuteAll_24rounds(KeccakP1600times4_SIMD256_states *states);
void KeccakP1600times4_AVX2_ExtractBytes(const KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, unsigned char *data, unsigned int offset, unsigned int length);
void KeccakP1600times4_AVX2_ExtractLanesAll(const KeccakP1600times4_SIMD256_states *states, unsigned char *data, unsigned int laneCount, unsigned int laneOffset);
void KeccakP1600times4_AVX2_ExtractAndAddBytes(const KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex,  const unsigned char *input, unsigned char *output, unsigned int offset, unsigned int length);
void KeccakP1600times4_AVX2_ExtractAndAddLanesAll(const KeccakP1600times4_SIMD256_states *states, const unsigned char *input, unsigned char *output, unsigned int laneCount, unsigned int laneOffset);

size_t KeccakF1600times4_AVX2_FastLoop_Absorb(KeccakP1600times4_SIMD256_states *states, unsigned int laneCount, unsigned int laneOffsetParallel, unsigned int laneOffsetSerial, const unsigned char *data, size_t dataByteLen);
size_t KeccakP1600times4_12rounds_AVX2_FastLoop_Absorb(KeccakP1600times4_SIMD256_states *states, unsigned int laneCount, unsigned int laneOffsetParallel, unsigned int laneOffsetSerial, const unsigned char *data, size_t dataByteLen);

size_t KeccakP1600times4_AVX2_KravatteCompress(uint64_t *xAccu, uint64_t *kRoll, const unsigned char *input, size_t inputByteLen);
size_t KeccakP1600times4_AVX2_KravatteExpand(uint64_t *yAccu, const uint64_t *kRoll, unsigned char *output, size_t outputByteLen);

void KeccakP1600times4_AVX2_KT128ProcessLeaves(const unsigned char *input, unsigned char *output);
void KeccakP1600times4_AVX2_KT256ProcessLeaves(const unsigned char *input, unsigned char *output);

#endif
