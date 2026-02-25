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

#ifndef _KeccakP_1600_times4_SnP_h_
#define _KeccakP_1600_times4_SnP_h_

#include "KeccakP-1600-times4-AVX2.h"

typedef KeccakP1600times4_SIMD256_states KeccakP1600times4_states;

#define KeccakP1600times4_GetImplementation         KeccakP1600times4_AVX2_GetImplementation
#define KeccakP1600times4_GetFeatures               KeccakP1600times4_AVX2_GetFeatures

#define KeccakP1600times4_StaticInitialize          KeccakP1600times4_AVX2_StaticInitialize
#define KeccakP1600times4_InitializeAll             KeccakP1600times4_AVX2_InitializeAll
#define KeccakP1600times4_AddByte                   KeccakP1600times4_AVX2_AddByte
#define KeccakP1600times4_AddBytes                  KeccakP1600times4_AVX2_AddBytes
#define KeccakP1600times4_AddLanesAll               KeccakP1600times4_AVX2_AddLanesAll
#define KeccakP1600times4_OverwriteBytes            KeccakP1600times4_AVX2_OverwriteBytes
#define KeccakP1600times4_OverwriteLanesAll         KeccakP1600times4_AVX2_OverwriteLanesAll
#define KeccakP1600times4_OverwriteWithZeroes       KeccakP1600times4_AVX2_OverwriteWithZeroes
#define KeccakP1600times4_PermuteAll_4rounds        KeccakP1600times4_AVX2_PermuteAll_4rounds
#define KeccakP1600times4_PermuteAll_6rounds        KeccakP1600times4_AVX2_PermuteAll_6rounds
#define KeccakP1600times4_PermuteAll_12rounds       KeccakP1600times4_AVX2_PermuteAll_12rounds
#define KeccakP1600times4_PermuteAll_24rounds       KeccakP1600times4_AVX2_PermuteAll_24rounds
#define KeccakP1600times4_ExtractBytes              KeccakP1600times4_AVX2_ExtractBytes
#define KeccakP1600times4_ExtractLanesAll           KeccakP1600times4_AVX2_ExtractLanesAll
#define KeccakP1600times4_ExtractAndAddBytes        KeccakP1600times4_AVX2_ExtractAndAddBytes
#define KeccakP1600times4_ExtractAndAddLanesAll     KeccakP1600times4_AVX2_ExtractAndAddLanesAll

#define KeccakF1600times4_FastLoop_Absorb           KeccakF1600times4_AVX2_FastLoop_Absorb
#define KeccakP1600times4_12rounds_FastLoop_Absorb  KeccakP1600times4_12rounds_AVX2_FastLoop_Absorb

#define KeccakP1600times4_KravatteCompress          KeccakP1600times4_AVX2_KravatteCompress
#define KeccakP1600times4_KravatteExpand            KeccakP1600times4_AVX2_KravatteExpand

#define KeccakP1600times4_KT128ProcessLeaves        KeccakP1600times4_AVX2_KT128ProcessLeaves
#define KeccakP1600times4_KT256ProcessLeaves        KeccakP1600times4_AVX2_KT256ProcessLeaves

#endif
