/*
The Keccak-p permutations, designed by Guido Bertoni, Joan Daemen, Michaël Peeters and Gilles Van Assche.

Implementation by Gilles Van Assche and Ronny Van Keer, hereby denoted as "the implementer".

For more information, feedback or questions, please refer to the Keccak Team website:
https://keccak.team/

To the extent possible under law, the implementer has waived all copyright
and related or neighboring rights to the source code in this file.
http://creativecommons.org/publicdomain/zero/1.0/

---

This file implements Keccak-p[1600]×4 in a PlSnP-compatible way.
Please refer to PlSnP-documentation.h for more details.

This implementation comes with KeccakP-1600-times4-SnP.h in the same folder.
Please refer to LowLevel.build for the exact list of other files it must be combined with.
*/

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <emmintrin.h>
#include "align.h"
#include "KeccakP-1600-times4-SnP.h"

#include "brg_endian.h"
#if (PLATFORM_BYTE_ORDER != IS_LITTLE_ENDIAN)
#error Expecting a little-endian platform
#endif

typedef __m128i V128;

//#define UseGatherScatter

#define laneIndex(instanceIndex, lanePosition) ((lanePosition)*4 + instanceIndex)

#define ANDnu256(a, b)          _mm256_andnot_si256(a, b)
#define CONST256(a)             _mm256_load_si256((const V256 *)&(a))
#define CONST256_64(a)          _mm256_set1_epi64x(a)
#define LOAD256(a)              _mm256_load_si256((const V256 *)&(a))
#define LOAD256u(a)             _mm256_loadu_si256((const V256 *)&(a))
#define LOAD4_64(a, b, c, d)    _mm256_set_epi64x((uint64_t)(a), (uint64_t)(b), (uint64_t)(c), (uint64_t)(d))
#define ROL64in256(d, a, o)     d = _mm256_or_si256(_mm256_slli_epi64(a, o), _mm256_srli_epi64(a, 64-(o)))
#define ROL64in256_8(d, a)      d = _mm256_shuffle_epi8(a, CONST256(rho8))
#define ROL64in256_56(d, a)     d = _mm256_shuffle_epi8(a, CONST256(rho56))
static ALIGN(32) const uint64_t rho8[4] = {0x0605040302010007, 0x0E0D0C0B0A09080F, 0x1615141312111017, 0x1E1D1C1B1A19181F};
static ALIGN(32) const uint64_t rho56[4] = {0x0007060504030201, 0x080F0E0D0C0B0A09, 0x1017161514131211, 0x181F1E1D1C1B1A19};
#define STORE256(a, b)          _mm256_store_si256((V256 *)&(a), b)
#define STORE256u(a, b)         _mm256_storeu_si256((V256 *)&(a), b)
#define STORE2_128(ah, al, v)   _mm256_storeu2_m128i(&(ah), &(al), v)
#define XOR256(a, b)            _mm256_xor_si256(a, b)
#define XOReq256(a, b)          a = _mm256_xor_si256(a, b)
#define UNPACKL( a, b )         _mm256_unpacklo_epi64((a), (b))
#define UNPACKH( a, b )         _mm256_unpackhi_epi64((a), (b))
#define PERM128( a, b, c )      _mm256_permute2f128_si256((a), (b), c)
#define SHUFFLE64( a, b, c )    _mm256_castpd_si256(_mm256_shuffle_pd(_mm256_castsi256_pd(a), _mm256_castsi256_pd(b), c))
#define ZERO()                  _mm256_setzero_si256()

#define UNINTLEAVE()            lanesL01 = UNPACKL( lanes0, lanes1 ),                   \
                                lanesH01 = UNPACKH( lanes0, lanes1 ),                   \
                                lanesL23 = UNPACKL( lanes2, lanes3 ),                   \
                                lanesH23 = UNPACKH( lanes2, lanes3 ),                   \
                                lanes0 = PERM128( lanesL01, lanesL23, 0x20 ),           \
                                lanes2 = PERM128( lanesL01, lanesL23, 0x31 ),           \
                                lanes1 = PERM128( lanesH01, lanesH23, 0x20 ),           \
                                lanes3 = PERM128( lanesH01, lanesH23, 0x31 )

#define INTLEAVE()              lanesL01 = PERM128( lanes0, lanes2, 0x20 ),             \
                                lanesH01 = PERM128( lanes1, lanes3, 0x20 ),             \
                                lanesL23 = PERM128( lanes0, lanes2, 0x31 ),             \
                                lanesH23 = PERM128( lanes1, lanes3, 0x31 ),             \
                                lanes0 = SHUFFLE64( lanesL01, lanesH01, 0x00 ),         \
                                lanes1 = SHUFFLE64( lanesL01, lanesH01, 0x0F ),         \
                                lanes2 = SHUFFLE64( lanesL23, lanesH23, 0x00 ),         \
                                lanes3 = SHUFFLE64( lanesL23, lanesH23, 0x0F )

#define SnP_laneLengthInBytes 8

void KeccakP1600times4_AVX2_InitializeAll(KeccakP1600times4_SIMD256_states *states)
{
    memset(states, 0, sizeof(KeccakP1600times4_SIMD256_states));
}

void KeccakP1600times4_AVX2_AddBytes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, const unsigned char *data, unsigned int offset, unsigned int length)
{
    unsigned int sizeLeft = length;
    unsigned int lanePosition = offset/SnP_laneLengthInBytes;
    unsigned int offsetInLane = offset%SnP_laneLengthInBytes;
    const unsigned char *curData = data;
    uint64_t *statesAsLanes = (uint64_t *)states->A;

    if ((sizeLeft > 0) && (offsetInLane != 0)) {
        unsigned int bytesInLane = SnP_laneLengthInBytes - offsetInLane;
        uint64_t lane = 0;
        if (bytesInLane > sizeLeft)
            bytesInLane = sizeLeft;
        memcpy((unsigned char*)&lane + offsetInLane, curData, bytesInLane);
        statesAsLanes[laneIndex(instanceIndex, lanePosition)] ^= lane;
        sizeLeft -= bytesInLane;
        lanePosition++;
        curData += bytesInLane;
    }

    while(sizeLeft >= SnP_laneLengthInBytes) {
        uint64_t lane = *((const uint64_t*)curData);
        statesAsLanes[laneIndex(instanceIndex, lanePosition)] ^= lane;
        sizeLeft -= SnP_laneLengthInBytes;
        lanePosition++;
        curData += SnP_laneLengthInBytes;
    }

    if (sizeLeft > 0) {
        uint64_t lane = 0;
        memcpy(&lane, curData, sizeLeft);
        statesAsLanes[laneIndex(instanceIndex, lanePosition)] ^= lane;
    }
}

void KeccakP1600times4_AVX2_AddLanesAll(KeccakP1600times4_SIMD256_states *states, const unsigned char *data, unsigned int laneCount, unsigned int laneOffset)
{
    V256 *stateAsLanes = states->A;
    unsigned int i;
    const uint64_t *curData0 = (const uint64_t *)data;
    const uint64_t *curData1 = (const uint64_t *)(data+laneOffset*SnP_laneLengthInBytes);
    const uint64_t *curData2 = (const uint64_t *)(data+laneOffset*2*SnP_laneLengthInBytes);
    const uint64_t *curData3 = (const uint64_t *)(data+laneOffset*3*SnP_laneLengthInBytes);
    V256    lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;

    #define Xor_In( argIndex )  XOReq256(stateAsLanes[argIndex], LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))

    #define Xor_In4( argIndex ) lanes0 = LOAD256u( curData0[argIndex]),\
                                lanes1 = LOAD256u( curData1[argIndex]),\
                                lanes2 = LOAD256u( curData2[argIndex]),\
                                lanes3 = LOAD256u( curData3[argIndex]),\
                                INTLEAVE(),\
                                XOReq256( stateAsLanes[argIndex+0], lanes0 ),\
                                XOReq256( stateAsLanes[argIndex+1], lanes1 ),\
                                XOReq256( stateAsLanes[argIndex+2], lanes2 ),\
                                XOReq256( stateAsLanes[argIndex+3], lanes3 )

    if ( laneCount >= 16 )  {
        Xor_In4( 0 );
        Xor_In4( 4 );
        Xor_In4( 8 );
        Xor_In4( 12 );
        if ( laneCount >= 20 )  {
            Xor_In4( 16 );
            for(i=20; i<laneCount; i++)
                Xor_In( i );
        }
        else {
            for(i=16; i<laneCount; i++)
                Xor_In( i );
        }
    }
    else {
        for(i=0; i<laneCount; i++)
            Xor_In( i );
    }
    #undef  Xor_In
    #undef  Xor_In4
}

void KeccakP1600times4_AVX2_OverwriteBytes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, const unsigned char *data, unsigned int offset, unsigned int length)
{
    unsigned int sizeLeft = length;
    unsigned int lanePosition = offset/SnP_laneLengthInBytes;
    unsigned int offsetInLane = offset%SnP_laneLengthInBytes;
    const unsigned char *curData = data;
    uint64_t *statesAsLanes = (uint64_t *)states->A;

    if ((sizeLeft > 0) && (offsetInLane != 0)) {
        unsigned int bytesInLane = SnP_laneLengthInBytes - offsetInLane;
        if (bytesInLane > sizeLeft)
            bytesInLane = sizeLeft;
        memcpy( ((unsigned char *)&statesAsLanes[laneIndex(instanceIndex, lanePosition)]) + offsetInLane, curData, bytesInLane);
        sizeLeft -= bytesInLane;
        lanePosition++;
        curData += bytesInLane;
    }

    while(sizeLeft >= SnP_laneLengthInBytes) {
        uint64_t lane = *((const uint64_t*)curData);
        statesAsLanes[laneIndex(instanceIndex, lanePosition)] = lane;
        sizeLeft -= SnP_laneLengthInBytes;
        lanePosition++;
        curData += SnP_laneLengthInBytes;
    }

    if (sizeLeft > 0) {
        memcpy(&statesAsLanes[laneIndex(instanceIndex, lanePosition)], curData, sizeLeft);
    }
}

void KeccakP1600times4_AVX2_OverwriteLanesAll(KeccakP1600times4_SIMD256_states *states, const unsigned char *data, unsigned int laneCount, unsigned int laneOffset)
{
    V256 *stateAsLanes = states->A;
    unsigned int i;
    const uint64_t *curData0 = (const uint64_t *)data;
    const uint64_t *curData1 = (const uint64_t *)(data+laneOffset*SnP_laneLengthInBytes);
    const uint64_t *curData2 = (const uint64_t *)(data+laneOffset*2*SnP_laneLengthInBytes);
    const uint64_t *curData3 = (const uint64_t *)(data+laneOffset*3*SnP_laneLengthInBytes);
    V256    lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;

    #define OverWr( argIndex )  STORE256(stateAsLanes[argIndex], LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))

    #define OverWr4( argIndex )     lanes0 = LOAD256u( curData0[argIndex]),\
                                    lanes1 = LOAD256u( curData1[argIndex]),\
                                    lanes2 = LOAD256u( curData2[argIndex]),\
                                    lanes3 = LOAD256u( curData3[argIndex]),\
                                    INTLEAVE(),\
                                    STORE256( stateAsLanes[argIndex+0], lanes0 ),\
                                    STORE256( stateAsLanes[argIndex+1], lanes1 ),\
                                    STORE256( stateAsLanes[argIndex+2], lanes2 ),\
                                    STORE256( stateAsLanes[argIndex+3], lanes3 )

    if ( laneCount >= 16 )  {
        OverWr4( 0 );
        OverWr4( 4 );
        OverWr4( 8 );
        OverWr4( 12 );
        if ( laneCount >= 20 )  {
            OverWr4( 16 );
            for(i=20; i<laneCount; i++)
                OverWr( i );
        }
        else {
            for(i=16; i<laneCount; i++)
                OverWr( i );
        }
    }
    else {
        for(i=0; i<laneCount; i++)
            OverWr( i );
    }
    #undef  OverWr
    #undef  OverWr4
}

void KeccakP1600times4_AVX2_OverwriteWithZeroes(KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, unsigned int byteCount)
{
    unsigned int sizeLeft = byteCount;
    unsigned int lanePosition = 0;
    uint64_t *statesAsLanes = (uint64_t *)states->A;

    while(sizeLeft >= SnP_laneLengthInBytes) {
        statesAsLanes[laneIndex(instanceIndex, lanePosition)] = 0;
        sizeLeft -= SnP_laneLengthInBytes;
        lanePosition++;
    }

    if (sizeLeft > 0) {
        memset(&statesAsLanes[laneIndex(instanceIndex, lanePosition)], 0, sizeLeft);
    }
}

void KeccakP1600times4_AVX2_ExtractBytes(const KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, unsigned char *data, unsigned int offset, unsigned int length)
{
    unsigned int sizeLeft = length;
    unsigned int lanePosition = offset/SnP_laneLengthInBytes;
    unsigned int offsetInLane = offset%SnP_laneLengthInBytes;
    unsigned char *curData = data;
    const uint64_t *statesAsLanes = (const uint64_t *)states->A;

    if ((sizeLeft > 0) && (offsetInLane != 0)) {
        unsigned int bytesInLane = SnP_laneLengthInBytes - offsetInLane;
        if (bytesInLane > sizeLeft)
            bytesInLane = sizeLeft;
        memcpy( curData, ((unsigned char *)&statesAsLanes[laneIndex(instanceIndex, lanePosition)]) + offsetInLane, bytesInLane);
        sizeLeft -= bytesInLane;
        lanePosition++;
        curData += bytesInLane;
    }

    while(sizeLeft >= SnP_laneLengthInBytes) {
        *(uint64_t*)curData = statesAsLanes[laneIndex(instanceIndex, lanePosition)];
        sizeLeft -= SnP_laneLengthInBytes;
        lanePosition++;
        curData += SnP_laneLengthInBytes;
    }

    if (sizeLeft > 0) {
        memcpy( curData, &statesAsLanes[laneIndex(instanceIndex, lanePosition)], sizeLeft);
    }
}

void KeccakP1600times4_AVX2_ExtractLanesAll(const KeccakP1600times4_SIMD256_states *states, unsigned char *data, unsigned int laneCount, unsigned int laneOffset)
{
    uint64_t *curData0 = (uint64_t *)data;
    uint64_t *curData1 = (uint64_t *)(data+laneOffset*1*SnP_laneLengthInBytes);
    uint64_t *curData2 = (uint64_t *)(data+laneOffset*2*SnP_laneLengthInBytes);
    uint64_t *curData3 = (uint64_t *)(data+laneOffset*3*SnP_laneLengthInBytes);

    const V256 *stateAsLanes = states->A;
    const uint64_t *stateAsLanes64 = (const uint64_t*)states->A;
    V256    lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;
    unsigned int i;

    #define Extr( argIndex )    curData0[argIndex] = stateAsLanes64[4*(argIndex)],      \
                                curData1[argIndex] = stateAsLanes64[4*(argIndex)+1],    \
                                curData2[argIndex] = stateAsLanes64[4*(argIndex)+2],    \
                                curData3[argIndex] = stateAsLanes64[4*(argIndex)+3]

    #define Extr4( argIndex )   lanes0 = LOAD256( stateAsLanes[argIndex+0] ),           \
                                lanes1 = LOAD256( stateAsLanes[argIndex+1] ),           \
                                lanes2 = LOAD256( stateAsLanes[argIndex+2] ),           \
                                lanes3 = LOAD256( stateAsLanes[argIndex+3] ),           \
                                UNINTLEAVE(),                                           \
                                STORE256u( curData0[argIndex], lanes0 ),                \
                                STORE256u( curData1[argIndex], lanes1 ),                \
                                STORE256u( curData2[argIndex], lanes2 ),                \
                                STORE256u( curData3[argIndex], lanes3 )

    if ( laneCount >= 16 )  {
        Extr4( 0 );
        Extr4( 4 );
        Extr4( 8 );
        Extr4( 12 );
        if ( laneCount >= 20 )  {
            Extr4( 16 );
            for(i=20; i<laneCount; i++)
                Extr( i );
        }
        else {
            for(i=16; i<laneCount; i++)
                Extr( i );
        }
    }
    else {
        for(i=0; i<laneCount; i++)
            Extr( i );
    }
    #undef  Extr
    #undef  Extr4
}

void KeccakP1600times4_AVX2_ExtractAndAddBytes(const KeccakP1600times4_SIMD256_states *states, unsigned int instanceIndex, const unsigned char *input, unsigned char *output, unsigned int offset, unsigned int length)
{
    unsigned int sizeLeft = length;
    unsigned int lanePosition = offset/SnP_laneLengthInBytes;
    unsigned int offsetInLane = offset%SnP_laneLengthInBytes;
    const unsigned char *curInput = input;
    unsigned char *curOutput = output;
    const uint64_t *statesAsLanes = (const uint64_t *)states->A;

    if ((sizeLeft > 0) && (offsetInLane != 0)) {
        unsigned int bytesInLane = SnP_laneLengthInBytes - offsetInLane;
        uint64_t lane = statesAsLanes[laneIndex(instanceIndex, lanePosition)] >> (8 * offsetInLane);
        if (bytesInLane > sizeLeft)
            bytesInLane = sizeLeft;
        sizeLeft -= bytesInLane;
        do {
            *(curOutput++) = *(curInput++) ^ (unsigned char)lane;
            lane >>= 8;
        } while ( --bytesInLane != 0);
        lanePosition++;
    }

    while(sizeLeft >= SnP_laneLengthInBytes) {
        *((uint64_t*)curOutput) = *((uint64_t*)curInput) ^ statesAsLanes[laneIndex(instanceIndex, lanePosition)];
        sizeLeft -= SnP_laneLengthInBytes;
        lanePosition++;
        curInput += SnP_laneLengthInBytes;
        curOutput += SnP_laneLengthInBytes;
    }

    if (sizeLeft != 0) {
        uint64_t lane = statesAsLanes[laneIndex(instanceIndex, lanePosition)];
        do {
            *(curOutput++) = *(curInput++) ^ (unsigned char)lane;
            lane >>= 8;
        } while ( --sizeLeft != 0);
    }
}

void KeccakP1600times4_AVX2_ExtractAndAddLanesAll(const KeccakP1600times4_SIMD256_states *states, const unsigned char *input, unsigned char *output, unsigned int laneCount, unsigned int laneOffset)
{
    const uint64_t *curInput0 = (uint64_t *)input;
    const uint64_t *curInput1 = (uint64_t *)(input+laneOffset*1*SnP_laneLengthInBytes);
    const uint64_t *curInput2 = (uint64_t *)(input+laneOffset*2*SnP_laneLengthInBytes);
    const uint64_t *curInput3 = (uint64_t *)(input+laneOffset*3*SnP_laneLengthInBytes);
    uint64_t *curOutput0 = (uint64_t *)output;
    uint64_t *curOutput1 = (uint64_t *)(output+laneOffset*1*SnP_laneLengthInBytes);
    uint64_t *curOutput2 = (uint64_t *)(output+laneOffset*2*SnP_laneLengthInBytes);
    uint64_t *curOutput3 = (uint64_t *)(output+laneOffset*3*SnP_laneLengthInBytes);

    const V256 *stateAsLanes = states->A;
    const uint64_t *stateAsLanes64 = (const uint64_t*)states->A;
    V256    lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;
    unsigned int i;

    #define ExtrXor( argIndex ) \
                                curOutput0[argIndex] = curInput0[argIndex] ^ stateAsLanes64[4*(argIndex)],\
                                curOutput1[argIndex] = curInput1[argIndex] ^ stateAsLanes64[4*(argIndex)+1],\
                                curOutput2[argIndex] = curInput2[argIndex] ^ stateAsLanes64[4*(argIndex)+2],\
                                curOutput3[argIndex] = curInput3[argIndex] ^ stateAsLanes64[4*(argIndex)+3]

    #define ExtrXor4( argIndex ) \
                                    lanes0 = LOAD256( stateAsLanes[argIndex+0] ),\
                                    lanes1 = LOAD256( stateAsLanes[argIndex+1] ),\
                                    lanes2 = LOAD256( stateAsLanes[argIndex+2] ),\
                                    lanes3 = LOAD256( stateAsLanes[argIndex+3] ),\
                                    UNINTLEAVE(),\
                                    lanesL01 = LOAD256u( curInput0[argIndex]),\
                                    lanesH01 = LOAD256u( curInput1[argIndex]),\
                                    lanesL23 = LOAD256u( curInput2[argIndex]),\
                                    lanesH23 = LOAD256u( curInput3[argIndex]),\
                                    XOReq256( lanes0, lanesL01 ),\
                                    XOReq256( lanes1, lanesH01 ),\
                                    XOReq256( lanes2, lanesL23 ),\
                                    XOReq256( lanes3, lanesH23 ),\
                                    STORE256u( curOutput0[argIndex], lanes0 ),\
                                    STORE256u( curOutput1[argIndex], lanes1 ),\
                                    STORE256u( curOutput2[argIndex], lanes2 ),\
                                    STORE256u( curOutput3[argIndex], lanes3 )

    if ( laneCount >= 16 )  {
        ExtrXor4( 0 );
        ExtrXor4( 4 );
        ExtrXor4( 8 );
        ExtrXor4( 12 );
        if ( laneCount >= 20 )  {
            ExtrXor4( 16 );
            for(i=20; i<laneCount; i++)
                ExtrXor( i );
        }
        else {
            for(i=16; i<laneCount; i++)
                ExtrXor( i );
        }
    }
    else {
        for(i=0; i<laneCount; i++)
            ExtrXor( i );
    }
    #undef  ExtrXor
    #undef  ExtrXor4
}

#define declareABCDE \
    V256 Aba, Abe, Abi, Abo, Abu; \
    V256 Aga, Age, Agi, Ago, Agu; \
    V256 Aka, Ake, Aki, Ako, Aku; \
    V256 Ama, Ame, Ami, Amo, Amu; \
    V256 Asa, Ase, Asi, Aso, Asu; \
    V256 Bba, Bbe, Bbi, Bbo, Bbu; \
    V256 Bga, Bge, Bgi, Bgo, Bgu; \
    V256 Bka, Bke, Bki, Bko, Bku; \
    V256 Bma, Bme, Bmi, Bmo, Bmu; \
    V256 Bsa, Bse, Bsi, Bso, Bsu; \
    V256 Ca, Ce, Ci, Co, Cu; \
    V256 Ca1, Ce1, Ci1, Co1, Cu1; \
    V256 Da, De, Di, Do, Du; \
    V256 Eba, Ebe, Ebi, Ebo, Ebu; \
    V256 Ega, Ege, Egi, Ego, Egu; \
    V256 Eka, Eke, Eki, Eko, Eku; \
    V256 Ema, Eme, Emi, Emo, Emu; \
    V256 Esa, Ese, Esi, Eso, Esu; \

#define prepareTheta \
    Ca = XOR256(Aba, XOR256(Aga, XOR256(Aka, XOR256(Ama, Asa)))); \
    Ce = XOR256(Abe, XOR256(Age, XOR256(Ake, XOR256(Ame, Ase)))); \
    Ci = XOR256(Abi, XOR256(Agi, XOR256(Aki, XOR256(Ami, Asi)))); \
    Co = XOR256(Abo, XOR256(Ago, XOR256(Ako, XOR256(Amo, Aso)))); \
    Cu = XOR256(Abu, XOR256(Agu, XOR256(Aku, XOR256(Amu, Asu)))); \

/* --- Theta Rho Pi Chi Iota Prepare-theta */
/* --- 64-bit lanes mapped to 64-bit words */
#define thetaRhoPiChiIotaPrepareTheta(i, A, E) \
    ROL64in256(Ce1, Ce, 1); \
    Da = XOR256(Cu, Ce1); \
    ROL64in256(Ci1, Ci, 1); \
    De = XOR256(Ca, Ci1); \
    ROL64in256(Co1, Co, 1); \
    Di = XOR256(Ce, Co1); \
    ROL64in256(Cu1, Cu, 1); \
    Do = XOR256(Ci, Cu1); \
    ROL64in256(Ca1, Ca, 1); \
    Du = XOR256(Co, Ca1); \
\
    XOReq256(A##ba, Da); \
    Bba = A##ba; \
    XOReq256(A##ge, De); \
    ROL64in256(Bbe, A##ge, 44); \
    XOReq256(A##ki, Di); \
    ROL64in256(Bbi, A##ki, 43); \
    E##ba = XOR256(Bba, ANDnu256(Bbe, Bbi)); \
    XOReq256(E##ba, CONST256_64(KeccakF1600RoundConstants[i])); \
    Ca = E##ba; \
    XOReq256(A##mo, Do); \
    ROL64in256(Bbo, A##mo, 21); \
    E##be = XOR256(Bbe, ANDnu256(Bbi, Bbo)); \
    Ce = E##be; \
    XOReq256(A##su, Du); \
    ROL64in256(Bbu, A##su, 14); \
    E##bi = XOR256(Bbi, ANDnu256(Bbo, Bbu)); \
    Ci = E##bi; \
    E##bo = XOR256(Bbo, ANDnu256(Bbu, Bba)); \
    Co = E##bo; \
    E##bu = XOR256(Bbu, ANDnu256(Bba, Bbe)); \
    Cu = E##bu; \
\
    XOReq256(A##bo, Do); \
    ROL64in256(Bga, A##bo, 28); \
    XOReq256(A##gu, Du); \
    ROL64in256(Bge, A##gu, 20); \
    XOReq256(A##ka, Da); \
    ROL64in256(Bgi, A##ka, 3); \
    E##ga = XOR256(Bga, ANDnu256(Bge, Bgi)); \
    XOReq256(Ca, E##ga); \
    XOReq256(A##me, De); \
    ROL64in256(Bgo, A##me, 45); \
    E##ge = XOR256(Bge, ANDnu256(Bgi, Bgo)); \
    XOReq256(Ce, E##ge); \
    XOReq256(A##si, Di); \
    ROL64in256(Bgu, A##si, 61); \
    E##gi = XOR256(Bgi, ANDnu256(Bgo, Bgu)); \
    XOReq256(Ci, E##gi); \
    E##go = XOR256(Bgo, ANDnu256(Bgu, Bga)); \
    XOReq256(Co, E##go); \
    E##gu = XOR256(Bgu, ANDnu256(Bga, Bge)); \
    XOReq256(Cu, E##gu); \
\
    XOReq256(A##be, De); \
    ROL64in256(Bka, A##be, 1); \
    XOReq256(A##gi, Di); \
    ROL64in256(Bke, A##gi, 6); \
    XOReq256(A##ko, Do); \
    ROL64in256(Bki, A##ko, 25); \
    E##ka = XOR256(Bka, ANDnu256(Bke, Bki)); \
    XOReq256(Ca, E##ka); \
    XOReq256(A##mu, Du); \
    ROL64in256_8(Bko, A##mu); \
    E##ke = XOR256(Bke, ANDnu256(Bki, Bko)); \
    XOReq256(Ce, E##ke); \
    XOReq256(A##sa, Da); \
    ROL64in256(Bku, A##sa, 18); \
    E##ki = XOR256(Bki, ANDnu256(Bko, Bku)); \
    XOReq256(Ci, E##ki); \
    E##ko = XOR256(Bko, ANDnu256(Bku, Bka)); \
    XOReq256(Co, E##ko); \
    E##ku = XOR256(Bku, ANDnu256(Bka, Bke)); \
    XOReq256(Cu, E##ku); \
\
    XOReq256(A##bu, Du); \
    ROL64in256(Bma, A##bu, 27); \
    XOReq256(A##ga, Da); \
    ROL64in256(Bme, A##ga, 36); \
    XOReq256(A##ke, De); \
    ROL64in256(Bmi, A##ke, 10); \
    E##ma = XOR256(Bma, ANDnu256(Bme, Bmi)); \
    XOReq256(Ca, E##ma); \
    XOReq256(A##mi, Di); \
    ROL64in256(Bmo, A##mi, 15); \
    E##me = XOR256(Bme, ANDnu256(Bmi, Bmo)); \
    XOReq256(Ce, E##me); \
    XOReq256(A##so, Do); \
    ROL64in256_56(Bmu, A##so); \
    E##mi = XOR256(Bmi, ANDnu256(Bmo, Bmu)); \
    XOReq256(Ci, E##mi); \
    E##mo = XOR256(Bmo, ANDnu256(Bmu, Bma)); \
    XOReq256(Co, E##mo); \
    E##mu = XOR256(Bmu, ANDnu256(Bma, Bme)); \
    XOReq256(Cu, E##mu); \
\
    XOReq256(A##bi, Di); \
    ROL64in256(Bsa, A##bi, 62); \
    XOReq256(A##go, Do); \
    ROL64in256(Bse, A##go, 55); \
    XOReq256(A##ku, Du); \
    ROL64in256(Bsi, A##ku, 39); \
    E##sa = XOR256(Bsa, ANDnu256(Bse, Bsi)); \
    XOReq256(Ca, E##sa); \
    XOReq256(A##ma, Da); \
    ROL64in256(Bso, A##ma, 41); \
    E##se = XOR256(Bse, ANDnu256(Bsi, Bso)); \
    XOReq256(Ce, E##se); \
    XOReq256(A##se, De); \
    ROL64in256(Bsu, A##se, 2); \
    E##si = XOR256(Bsi, ANDnu256(Bso, Bsu)); \
    XOReq256(Ci, E##si); \
    E##so = XOR256(Bso, ANDnu256(Bsu, Bsa)); \
    XOReq256(Co, E##so); \
    E##su = XOR256(Bsu, ANDnu256(Bsa, Bse)); \
    XOReq256(Cu, E##su); \
\

/* --- Theta Rho Pi Chi Iota */
/* --- 64-bit lanes mapped to 64-bit words */
#define thetaRhoPiChiIota(i, A, E) \
    ROL64in256(Ce1, Ce, 1); \
    Da = XOR256(Cu, Ce1); \
    ROL64in256(Ci1, Ci, 1); \
    De = XOR256(Ca, Ci1); \
    ROL64in256(Co1, Co, 1); \
    Di = XOR256(Ce, Co1); \
    ROL64in256(Cu1, Cu, 1); \
    Do = XOR256(Ci, Cu1); \
    ROL64in256(Ca1, Ca, 1); \
    Du = XOR256(Co, Ca1); \
\
    XOReq256(A##ba, Da); \
    Bba = A##ba; \
    XOReq256(A##ge, De); \
    ROL64in256(Bbe, A##ge, 44); \
    XOReq256(A##ki, Di); \
    ROL64in256(Bbi, A##ki, 43); \
    E##ba = XOR256(Bba, ANDnu256(Bbe, Bbi)); \
    XOReq256(E##ba, CONST256_64(KeccakF1600RoundConstants[i])); \
    XOReq256(A##mo, Do); \
    ROL64in256(Bbo, A##mo, 21); \
    E##be = XOR256(Bbe, ANDnu256(Bbi, Bbo)); \
    XOReq256(A##su, Du); \
    ROL64in256(Bbu, A##su, 14); \
    E##bi = XOR256(Bbi, ANDnu256(Bbo, Bbu)); \
    E##bo = XOR256(Bbo, ANDnu256(Bbu, Bba)); \
    E##bu = XOR256(Bbu, ANDnu256(Bba, Bbe)); \
\
    XOReq256(A##bo, Do); \
    ROL64in256(Bga, A##bo, 28); \
    XOReq256(A##gu, Du); \
    ROL64in256(Bge, A##gu, 20); \
    XOReq256(A##ka, Da); \
    ROL64in256(Bgi, A##ka, 3); \
    E##ga = XOR256(Bga, ANDnu256(Bge, Bgi)); \
    XOReq256(A##me, De); \
    ROL64in256(Bgo, A##me, 45); \
    E##ge = XOR256(Bge, ANDnu256(Bgi, Bgo)); \
    XOReq256(A##si, Di); \
    ROL64in256(Bgu, A##si, 61); \
    E##gi = XOR256(Bgi, ANDnu256(Bgo, Bgu)); \
    E##go = XOR256(Bgo, ANDnu256(Bgu, Bga)); \
    E##gu = XOR256(Bgu, ANDnu256(Bga, Bge)); \
\
    XOReq256(A##be, De); \
    ROL64in256(Bka, A##be, 1); \
    XOReq256(A##gi, Di); \
    ROL64in256(Bke, A##gi, 6); \
    XOReq256(A##ko, Do); \
    ROL64in256(Bki, A##ko, 25); \
    E##ka = XOR256(Bka, ANDnu256(Bke, Bki)); \
    XOReq256(A##mu, Du); \
    ROL64in256_8(Bko, A##mu); \
    E##ke = XOR256(Bke, ANDnu256(Bki, Bko)); \
    XOReq256(A##sa, Da); \
    ROL64in256(Bku, A##sa, 18); \
    E##ki = XOR256(Bki, ANDnu256(Bko, Bku)); \
    E##ko = XOR256(Bko, ANDnu256(Bku, Bka)); \
    E##ku = XOR256(Bku, ANDnu256(Bka, Bke)); \
\
    XOReq256(A##bu, Du); \
    ROL64in256(Bma, A##bu, 27); \
    XOReq256(A##ga, Da); \
    ROL64in256(Bme, A##ga, 36); \
    XOReq256(A##ke, De); \
    ROL64in256(Bmi, A##ke, 10); \
    E##ma = XOR256(Bma, ANDnu256(Bme, Bmi)); \
    XOReq256(A##mi, Di); \
    ROL64in256(Bmo, A##mi, 15); \
    E##me = XOR256(Bme, ANDnu256(Bmi, Bmo)); \
    XOReq256(A##so, Do); \
    ROL64in256_56(Bmu, A##so); \
    E##mi = XOR256(Bmi, ANDnu256(Bmo, Bmu)); \
    E##mo = XOR256(Bmo, ANDnu256(Bmu, Bma)); \
    E##mu = XOR256(Bmu, ANDnu256(Bma, Bme)); \
\
    XOReq256(A##bi, Di); \
    ROL64in256(Bsa, A##bi, 62); \
    XOReq256(A##go, Do); \
    ROL64in256(Bse, A##go, 55); \
    XOReq256(A##ku, Du); \
    ROL64in256(Bsi, A##ku, 39); \
    E##sa = XOR256(Bsa, ANDnu256(Bse, Bsi)); \
    XOReq256(A##ma, Da); \
    ROL64in256(Bso, A##ma, 41); \
    E##se = XOR256(Bse, ANDnu256(Bsi, Bso)); \
    XOReq256(A##se, De); \
    ROL64in256(Bsu, A##se, 2); \
    E##si = XOR256(Bsi, ANDnu256(Bso, Bsu)); \
    E##so = XOR256(Bso, ANDnu256(Bsu, Bsa)); \
    E##su = XOR256(Bsu, ANDnu256(Bsa, Bse)); \
\

static ALIGN(32) const uint64_t KeccakF1600RoundConstants[24] = {
    0x0000000000000001ULL,
    0x0000000000008082ULL,
    0x800000000000808aULL,
    0x8000000080008000ULL,
    0x000000000000808bULL,
    0x0000000080000001ULL,
    0x8000000080008081ULL,
    0x8000000000008009ULL,
    0x000000000000008aULL,
    0x0000000000000088ULL,
    0x0000000080008009ULL,
    0x000000008000000aULL,
    0x000000008000808bULL,
    0x800000000000008bULL,
    0x8000000000008089ULL,
    0x8000000000008003ULL,
    0x8000000000008002ULL,
    0x8000000000000080ULL,
    0x000000000000800aULL,
    0x800000008000000aULL,
    0x8000000080008081ULL,
    0x8000000000008080ULL,
    0x0000000080000001ULL,
    0x8000000080008008ULL};

#define copyFromState(X, state) \
    X##ba = LOAD256(state[ 0]); \
    X##be = LOAD256(state[ 1]); \
    X##bi = LOAD256(state[ 2]); \
    X##bo = LOAD256(state[ 3]); \
    X##bu = LOAD256(state[ 4]); \
    X##ga = LOAD256(state[ 5]); \
    X##ge = LOAD256(state[ 6]); \
    X##gi = LOAD256(state[ 7]); \
    X##go = LOAD256(state[ 8]); \
    X##gu = LOAD256(state[ 9]); \
    X##ka = LOAD256(state[10]); \
    X##ke = LOAD256(state[11]); \
    X##ki = LOAD256(state[12]); \
    X##ko = LOAD256(state[13]); \
    X##ku = LOAD256(state[14]); \
    X##ma = LOAD256(state[15]); \
    X##me = LOAD256(state[16]); \
    X##mi = LOAD256(state[17]); \
    X##mo = LOAD256(state[18]); \
    X##mu = LOAD256(state[19]); \
    X##sa = LOAD256(state[20]); \
    X##se = LOAD256(state[21]); \
    X##si = LOAD256(state[22]); \
    X##so = LOAD256(state[23]); \
    X##su = LOAD256(state[24]); \

#define copyToState(state, X) \
    STORE256(state[ 0], X##ba); \
    STORE256(state[ 1], X##be); \
    STORE256(state[ 2], X##bi); \
    STORE256(state[ 3], X##bo); \
    STORE256(state[ 4], X##bu); \
    STORE256(state[ 5], X##ga); \
    STORE256(state[ 6], X##ge); \
    STORE256(state[ 7], X##gi); \
    STORE256(state[ 8], X##go); \
    STORE256(state[ 9], X##gu); \
    STORE256(state[10], X##ka); \
    STORE256(state[11], X##ke); \
    STORE256(state[12], X##ki); \
    STORE256(state[13], X##ko); \
    STORE256(state[14], X##ku); \
    STORE256(state[15], X##ma); \
    STORE256(state[16], X##me); \
    STORE256(state[17], X##mi); \
    STORE256(state[18], X##mo); \
    STORE256(state[19], X##mu); \
    STORE256(state[20], X##sa); \
    STORE256(state[21], X##se); \
    STORE256(state[22], X##si); \
    STORE256(state[23], X##so); \
    STORE256(state[24], X##su); \

#define copyStateVariables(X, Y) \
    X##ba = Y##ba; \
    X##be = Y##be; \
    X##bi = Y##bi; \
    X##bo = Y##bo; \
    X##bu = Y##bu; \
    X##ga = Y##ga; \
    X##ge = Y##ge; \
    X##gi = Y##gi; \
    X##go = Y##go; \
    X##gu = Y##gu; \
    X##ka = Y##ka; \
    X##ke = Y##ke; \
    X##ki = Y##ki; \
    X##ko = Y##ko; \
    X##ku = Y##ku; \
    X##ma = Y##ma; \
    X##me = Y##me; \
    X##mi = Y##mi; \
    X##mo = Y##mo; \
    X##mu = Y##mu; \
    X##sa = Y##sa; \
    X##se = Y##se; \
    X##si = Y##si; \
    X##so = Y##so; \
    X##su = Y##su; \

 #ifdef KeccakP1600times4_AVX2_fullUnrolling
#define FullUnrolling
#else
#define Unrolling KeccakP1600times4_AVX2_unrolling
#endif
#include "KeccakP-1600-unrolling.macros"

void KeccakP1600times4_AVX2_PermuteAll_24rounds(KeccakP1600times4_SIMD256_states *states)
{
    V256 *statesAsLanes = states->A;
    declareABCDE
    #ifndef KeccakP1600times4_AVX2_fullUnrolling
    unsigned int i;
    #endif

    copyFromState(A, statesAsLanes)
    rounds24
    copyToState(statesAsLanes, A)
}

void KeccakP1600times4_AVX2_PermuteAll_12rounds(KeccakP1600times4_SIMD256_states *states)
{
    V256 *statesAsLanes = states->A;
    declareABCDE
    #ifndef KeccakP1600times4_AVX2_fullUnrolling
    unsigned int i;
    #endif

    copyFromState(A, statesAsLanes)
    rounds12
    copyToState(statesAsLanes, A)
}

void KeccakP1600times4_AVX2_PermuteAll_6rounds(KeccakP1600times4_SIMD256_states *states)
{
    V256 *statesAsLanes = states->A;
    declareABCDE
    #ifndef KeccakP1600times4_AVX2_fullUnrolling
    unsigned int i;
    #endif

    copyFromState(A, statesAsLanes)
    rounds6
    copyToState(statesAsLanes, A)
}

void KeccakP1600times4_AVX2_PermuteAll_4rounds(KeccakP1600times4_SIMD256_states *states)
{
    V256 *statesAsLanes = states->A;
    declareABCDE
    #ifndef KeccakP1600times4_AVX2_fullUnrolling
    unsigned int i;
    #endif

    copyFromState(A, statesAsLanes)
    rounds4
    copyToState(statesAsLanes, A)
}

size_t KeccakF1600times4_AVX2_FastLoop_Absorb(KeccakP1600times4_SIMD256_states *states, unsigned int laneCount, unsigned int laneOffsetParallel, unsigned int laneOffsetSerial, const unsigned char *data, size_t dataByteLen)
{
    if (laneCount == 21) {
#if 0
        const unsigned char *dataStart = data;
        const uint64_t *curData0 = (const uint64_t *)data;
        const uint64_t *curData1 = (const uint64_t *)(data+laneOffsetParallel*1*SnP_laneLengthInBytes);
        const uint64_t *curData2 = (const uint64_t *)(data+laneOffsetParallel*2*SnP_laneLengthInBytes);
        const uint64_t *curData3 = (const uint64_t *)(data+laneOffsetParallel*3*SnP_laneLengthInBytes);

        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            V256 *stateAsLanes = states->A;
            V256 lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;
            #define Xor_In( argIndex ) \
                XOReq256(stateAsLanes[argIndex], LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))
            #define Xor_In4( argIndex ) \
                lanes0 = LOAD256u( curData0[argIndex]),\
                lanes1 = LOAD256u( curData1[argIndex]),\
                lanes2 = LOAD256u( curData2[argIndex]),\
                lanes3 = LOAD256u( curData3[argIndex]),\
                INTLEAVE(),\
                XOReq256( stateAsLanes[argIndex+0], lanes0 ),\
                XOReq256( stateAsLanes[argIndex+1], lanes1 ),\
                XOReq256( stateAsLanes[argIndex+2], lanes2 ),\
                XOReq256( stateAsLanes[argIndex+3], lanes3 )
            Xor_In4( 0 );
            Xor_In4( 4 );
            Xor_In4( 8 );
            Xor_In4( 12 );
            Xor_In4( 16 );
            Xor_In( 20 );
            #undef  Xor_In
            #undef  Xor_In4
            KeccakP1600times4_AVX2_PermuteAll_24rounds(states);
            curData0 += laneOffsetSerial;
            curData1 += laneOffsetSerial;
            curData2 += laneOffsetSerial;
            curData3 += laneOffsetSerial;
            dataByteLen -= laneOffsetSerial*8;
        }
        return (const unsigned char *)curData0 - dataStart;
#else
        unsigned int i;
        const unsigned char *dataStart = data;
        const uint64_t *curData0 = (const uint64_t *)data;
        const uint64_t *curData1 = (const uint64_t *)(data+laneOffsetParallel*1*SnP_laneLengthInBytes);
        const uint64_t *curData2 = (const uint64_t *)(data+laneOffsetParallel*2*SnP_laneLengthInBytes);
        const uint64_t *curData3 = (const uint64_t *)(data+laneOffsetParallel*3*SnP_laneLengthInBytes);
        V256 *statesAsLanes = states->A;
        declareABCDE

        copyFromState(A, statesAsLanes)
        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            #define XOR_In( Xxx, argIndex ) \
                XOReq256(Xxx, LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))
            XOR_In( Aba, 0 );
            XOR_In( Abe, 1 );
            XOR_In( Abi, 2 );
            XOR_In( Abo, 3 );
            XOR_In( Abu, 4 );
            XOR_In( Aga, 5 );
            XOR_In( Age, 6 );
            XOR_In( Agi, 7 );
            XOR_In( Ago, 8 );
            XOR_In( Agu, 9 );
            XOR_In( Aka, 10 );
            XOR_In( Ake, 11 );
            XOR_In( Aki, 12 );
            XOR_In( Ako, 13 );
            XOR_In( Aku, 14 );
            XOR_In( Ama, 15 );
            XOR_In( Ame, 16 );
            XOR_In( Ami, 17 );
            XOR_In( Amo, 18 );
            XOR_In( Amu, 19 );
            XOR_In( Asa, 20 );
            #undef XOR_In
            rounds24
            curData0 += laneOffsetSerial;
            curData1 += laneOffsetSerial;
            curData2 += laneOffsetSerial;
            curData3 += laneOffsetSerial;
            dataByteLen -= laneOffsetSerial*8;
        }
        copyToState(statesAsLanes, A)
        return (const unsigned char *)curData0 - dataStart;
#endif
    }
    else {
        unsigned int i;
        const unsigned char *dataStart = data;

        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            KeccakP1600times4_AVX2_AddLanesAll(states, data, laneCount, laneOffsetParallel);
            KeccakP1600times4_AVX2_PermuteAll_24rounds(states);
            data += laneOffsetSerial*8;
            dataByteLen -= laneOffsetSerial*8;
        }
        return data - dataStart;
    }
}

size_t KeccakP1600times4_12rounds_AVX2_FastLoop_Absorb(KeccakP1600times4_SIMD256_states *states, unsigned int laneCount, unsigned int laneOffsetParallel, unsigned int laneOffsetSerial, const unsigned char *data, size_t dataByteLen)
{
    if (laneCount == 21) {
#if 0
        const unsigned char *dataStart = data;
        const uint64_t *curData0 = (const uint64_t *)data;
        const uint64_t *curData1 = (const uint64_t *)(data+laneOffsetParallel*1*SnP_laneLengthInBytes);
        const uint64_t *curData2 = (const uint64_t *)(data+laneOffsetParallel*2*SnP_laneLengthInBytes);
        const uint64_t *curData3 = (const uint64_t *)(data+laneOffsetParallel*3*SnP_laneLengthInBytes);

        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            V256 *stateAsLanes = states->A;
            V256 lanes0, lanes1, lanes2, lanes3, lanesL01, lanesL23, lanesH01, lanesH23;
            #define Xor_In( argIndex ) \
                XOReq256(stateAsLanes[argIndex], LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))
            #define Xor_In4( argIndex ) \
                lanes0 = LOAD256u( curData0[argIndex]),\
                lanes1 = LOAD256u( curData1[argIndex]),\
                lanes2 = LOAD256u( curData2[argIndex]),\
                lanes3 = LOAD256u( curData3[argIndex]),\
                INTLEAVE(),\
                XOReq256( stateAsLanes[argIndex+0], lanes0 ),\
                XOReq256( stateAsLanes[argIndex+1], lanes1 ),\
                XOReq256( stateAsLanes[argIndex+2], lanes2 ),\
                XOReq256( stateAsLanes[argIndex+3], lanes3 )
            Xor_In4( 0 );
            Xor_In4( 4 );
            Xor_In4( 8 );
            Xor_In4( 12 );
            Xor_In4( 16 );
            Xor_In( 20 );
            #undef  Xor_In
            #undef  Xor_In4
            KeccakP1600times4_AVX2_PermuteAll_12rounds(states);
            curData0 += laneOffsetSerial;
            curData1 += laneOffsetSerial;
            curData2 += laneOffsetSerial;
            curData3 += laneOffsetSerial;
            dataByteLen -= laneOffsetSerial*8;
        }
        return (const unsigned char *)curData0 - dataStart;
#else
        unsigned int i;
        const unsigned char *dataStart = data;
        const uint64_t *curData0 = (const uint64_t *)data;
        const uint64_t *curData1 = (const uint64_t *)(data+laneOffsetParallel*1*SnP_laneLengthInBytes);
        const uint64_t *curData2 = (const uint64_t *)(data+laneOffsetParallel*2*SnP_laneLengthInBytes);
        const uint64_t *curData3 = (const uint64_t *)(data+laneOffsetParallel*3*SnP_laneLengthInBytes);
        V256 *statesAsLanes = states->A;
        declareABCDE

        copyFromState(A, statesAsLanes)
        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            #define XOR_In( Xxx, argIndex ) \
                XOReq256(Xxx, LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))
            XOR_In( Aba, 0 );
            XOR_In( Abe, 1 );
            XOR_In( Abi, 2 );
            XOR_In( Abo, 3 );
            XOR_In( Abu, 4 );
            XOR_In( Aga, 5 );
            XOR_In( Age, 6 );
            XOR_In( Agi, 7 );
            XOR_In( Ago, 8 );
            XOR_In( Agu, 9 );
            XOR_In( Aka, 10 );
            XOR_In( Ake, 11 );
            XOR_In( Aki, 12 );
            XOR_In( Ako, 13 );
            XOR_In( Aku, 14 );
            XOR_In( Ama, 15 );
            XOR_In( Ame, 16 );
            XOR_In( Ami, 17 );
            XOR_In( Amo, 18 );
            XOR_In( Amu, 19 );
            XOR_In( Asa, 20 );
            #undef XOR_In
            rounds12
            curData0 += laneOffsetSerial;
            curData1 += laneOffsetSerial;
            curData2 += laneOffsetSerial;
            curData3 += laneOffsetSerial;
            dataByteLen -= laneOffsetSerial*8;
        }
        copyToState(statesAsLanes, A)
        return (const unsigned char *)curData0 - dataStart;
#endif
    }
    else if (laneCount == 17) {
        unsigned int i;
        const unsigned char *dataStart = data;
        const uint64_t *curData0 = (const uint64_t *)data;
        const uint64_t *curData1 = (const uint64_t *)(data+laneOffsetParallel*1*SnP_laneLengthInBytes);
        const uint64_t *curData2 = (const uint64_t *)(data+laneOffsetParallel*2*SnP_laneLengthInBytes);
        const uint64_t *curData3 = (const uint64_t *)(data+laneOffsetParallel*3*SnP_laneLengthInBytes);
        V256 *statesAsLanes = (V256*)states;
        declareABCDE

        copyFromState(A, statesAsLanes)
        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            #define XOR_In( Xxx, argIndex ) \
                XOReq256(Xxx, LOAD4_64(curData3[argIndex], curData2[argIndex], curData1[argIndex], curData0[argIndex]))
            XOR_In( Aba, 0 );
            XOR_In( Abe, 1 );
            XOR_In( Abi, 2 );
            XOR_In( Abo, 3 );
            XOR_In( Abu, 4 );
            XOR_In( Aga, 5 );
            XOR_In( Age, 6 );
            XOR_In( Agi, 7 );
            XOR_In( Ago, 8 );
            XOR_In( Agu, 9 );
            XOR_In( Aka, 10 );
            XOR_In( Ake, 11 );
            XOR_In( Aki, 12 );
            XOR_In( Ako, 13 );
            XOR_In( Aku, 14 );
            XOR_In( Ama, 15 );
            XOR_In( Ame, 16 );
            #undef XOR_In
            rounds12
            curData0 += laneOffsetSerial;
            curData1 += laneOffsetSerial;
            curData2 += laneOffsetSerial;
            curData3 += laneOffsetSerial;
            dataByteLen -= laneOffsetSerial*8;
        }
        copyToState(statesAsLanes, A)
        return (const unsigned char *)curData0 - dataStart;
    }
    else {
        unsigned int i;
        const unsigned char *dataStart = data;

        while(dataByteLen >= (laneOffsetParallel*3 + laneCount)*8) {
            KeccakP1600times4_AVX2_AddLanesAll(states, data, laneCount, laneOffsetParallel);
            KeccakP1600times4_AVX2_PermuteAll_12rounds(states);
            data += laneOffsetSerial*8;
            dataByteLen -= laneOffsetSerial*8;
        }
        return data - dataStart;
    }
}

/* ------------------------------------------------------------------------- */

#define UNINTLEAVEa(lanes0, lanes1, lanes2, lanes3)                                         \
                                    lanesL01 = UNPACKL( lanes0, lanes1 ),                   \
                                    lanesH01 = UNPACKH( lanes0, lanes1 ),                   \
                                    lanesL23 = UNPACKL( lanes2, lanes3 ),                   \
                                    lanesH23 = UNPACKH( lanes2, lanes3 ),                   \
                                    lanes0 = PERM128( lanesL01, lanesL23, 0x20 ),           \
                                    lanes2 = PERM128( lanesL01, lanesL23, 0x31 ),           \
                                    lanes1 = PERM128( lanesH01, lanesH23, 0x20 ),           \
                                    lanes3 = PERM128( lanesH01, lanesH23, 0x31 )

#define INTLEAVEa(lanes0, lanes1, lanes2, lanes3)                                           \
                                    lanesL01 = PERM128( lanes0, lanes2, 0x20 ),             \
                                    lanesH01 = PERM128( lanes1, lanes3, 0x20 ),             \
                                    lanesL23 = PERM128( lanes0, lanes2, 0x31 ),             \
                                    lanesH23 = PERM128( lanes1, lanes3, 0x31 ),             \
                                    lanes0 = SHUFFLE64( lanesL01, lanesH01, 0x00 ),         \
                                    lanes1 = SHUFFLE64( lanesL01, lanesH01, 0x0F ),         \
                                    lanes2 = SHUFFLE64( lanesL23, lanesH23, 0x00 ),         \
                                    lanes3 = SHUFFLE64( lanesL23, lanesH23, 0x0F )


#define LoadXOReq256( lanes, inp, argIndex)    XOReq256( lanes, LOAD4_64(inp[3*25+argIndex], inp[2*25+argIndex], inp[1*25+argIndex], inp[0*25+argIndex]) )

/* ------------------------------------------------------------------------- */

#if defined(UseGatherScatter)

#define AddOverWr4( lanes0, lanes1, lanes2, lanes3, key, inp, argIndex )                    \
                                    lanes0 = _mm256_i32gather_epi64((const long long int *)&inp[argIndex+0], gather, 1), \
                                    lanes1 = _mm256_i32gather_epi64((const long long int *)&inp[argIndex+1], gather, 1), \
                                    lanes2 = _mm256_i32gather_epi64((const long long int *)&inp[argIndex+2], gather, 1), \
                                    lanes3 = _mm256_i32gather_epi64((const long long int *)&inp[argIndex+3], gather, 1), \
                                    XOReq256( lanes0, CONST256_64( key[argIndex+0])),       \
                                    XOReq256( lanes1, CONST256_64( key[argIndex+1])),       \
                                    XOReq256( lanes2, CONST256_64( key[argIndex+2])),       \
                                    XOReq256( lanes3, CONST256_64( key[argIndex+3]))

#else

#define AddOverWr4( lanes0, lanes1, lanes2, lanes3, key, inp, argIndex )                    \
                                    lanes0 = LOAD256u( inp[argIndex+0*25]),                 \
                                    lanes1 = LOAD256u( inp[argIndex+1*25]),                 \
                                    lanes2 = LOAD256u( inp[argIndex+2*25]),                 \
                                    lanes3 = LOAD256u( inp[argIndex+3*25]),                 \
                                    INTLEAVEa(lanes0, lanes1, lanes2, lanes3),              \
                                    XOReq256( lanes0, CONST256_64( key[argIndex+0])),       \
                                    XOReq256( lanes1, CONST256_64( key[argIndex+1])),       \
                                    XOReq256( lanes2, CONST256_64( key[argIndex+2])),       \
                                    XOReq256( lanes3, CONST256_64( key[argIndex+3]))

#endif

#if defined(__i386__) || defined(_M_IX86)
#define _mm256_extract_epi64(a, index) \
    ((uint64_t)_mm256_extract_epi32((a), (index)*2) || ((uint64_t)_mm256_extract_epi32((a), (index)*2+1) << 32))
#endif

#define ExtrAccu( lanes, p, argIndex ) p[argIndex] ^= _mm256_extract_epi64(lanes, 0) ^ _mm256_extract_epi64(lanes, 1) \
                                                   ^  _mm256_extract_epi64(lanes, 2) ^ _mm256_extract_epi64(lanes, 3)

#define ExtrAccu4( lanes0, lanes1, lanes2, lanes3, p, argIndex )                            \
                                    UNINTLEAVEa(lanes0, lanes1, lanes2, lanes3),            \
                                    XOReq256( lanes0, lanes1 ),                             \
                                    XOReq256( lanes2, lanes3 ),                             \
                                    lanes1 = LOAD256u( p[argIndex]),                         \
                                    XOReq256( lanes0, lanes2 ),                             \
                                    XOReq256( lanes0, lanes1 ),                             \
                                    STORE256u( p[argIndex], lanes0 )

#define Kravatte_Rollc()                                                                                            \
    Asa = x0x1x2x3,                                                                                                 \
    Ase = x1x2x3x4,                                                                                                 \
    ROL64in256(x1x2x3x4, x0x1x2x3, 7),                                                                              \
    XOReq256(x1x2x3x4, Ase),                                                                                        \
    XOReq256(x1x2x3x4, _mm256_srli_epi64(Ase, 3)),                                                                  \
    Asi = _mm256_blend_epi32(_mm256_permute4x64_epi64(Ase, 0x39), _mm256_permute4x64_epi64(x1x2x3x4, 0x39), 0xC0),  \
    Aso = PERM128(Ase, x1x2x3x4, 0x21),                                                                             \
    Asu = _mm256_blend_epi32(_mm256_permute4x64_epi64(Ase, 0xFF), _mm256_permute4x64_epi64(x1x2x3x4, 0x90), 0xFC),  \
    x0x1x2x3 = Asu

size_t KeccakP1600times4_AVX2_KravatteCompress(uint64_t *xAccu, uint64_t *kRoll, const unsigned char *input, size_t inputByteLen)
{
    uint64_t *in64 = (uint64_t *)input;
    size_t    nBlocks = inputByteLen / (4 * 200);
    declareABCDE
    #if    !defined(KeccakP1600times4_AVX2_fullUnrolling)
    unsigned int i;
    #endif
    V256    lanesL01, lanesL23, lanesH01, lanesH23;
    V256    x0x1x2x3, x1x2x3x4;
    #if defined(UseGatherScatter)
    V128    gather = _mm_setr_epi32(0*25*8, 1*25*8, 2*25*8, 3*25*8);
    #endif

    x0x1x2x3 = LOAD256u(kRoll[20]);
    x1x2x3x4 = LOAD256u(kRoll[21]);
    do {
        AddOverWr4( Aba, Abe, Abi, Abo, kRoll, in64,  0 );
        AddOverWr4( Abu, Aga, Age, Agi, kRoll, in64,  4 );
        AddOverWr4( Ago, Agu, Aka, Ake, kRoll, in64,  8 );
        AddOverWr4( Aki, Ako, Aku, Ama, kRoll, in64, 12 );
        AddOverWr4( Ame, Ami, Amo, Amu, kRoll, in64, 16 );
        Kravatte_Rollc();
        LoadXOReq256(Asa, in64, 20);
        LoadXOReq256(Ase, in64, 21);
        LoadXOReq256(Asi, in64, 22);
        LoadXOReq256(Aso, in64, 23);
        LoadXOReq256(Asu, in64, 24);
        rounds6
        ExtrAccu4(Aba, Abe, Abi, Abo, xAccu,  0 );
        ExtrAccu4(Abu, Aga, Age, Agi, xAccu,  4 );
        ExtrAccu4(Ago, Agu, Aka, Ake, xAccu,  8 );
        ExtrAccu4(Aki, Ako, Aku, Ama, xAccu, 12 );
        ExtrAccu4(Ame, Ami, Amo, Amu, xAccu, 16 );
        ExtrAccu4(Asa, Ase, Asi, Aso, xAccu, 20 );
        ExtrAccu( Asu,                xAccu, 24 );
        in64 += 4 * 25;
    }
    while(--nBlocks != 0);
    STORE256u(kRoll[20], x0x1x2x3);
    kRoll[24] = _mm256_extract_epi64(x1x2x3x4, 3);

    return (size_t)in64 - (size_t)input;
}

#undef LoadXOReq256
#undef AddOverWr4
#undef ExtrAccu
#undef ExtrAccu4

/* ------------------------------------------------------------------------- */

#define ExtrAddKey( lanes, p, argIndex )                                                    \
                                    XOReq256(lanes, CONST256_64(kRoll[argIndex])),          \
                                    p[argIndex+0*25] = _mm256_extract_epi64(lanes, 0),      \
                                    p[argIndex+1*25] = _mm256_extract_epi64(lanes, 1),      \
                                    p[argIndex+2*25] = _mm256_extract_epi64(lanes, 2),      \
                                    p[argIndex+3*25] = _mm256_extract_epi64(lanes, 3)

#if 0//defined(UseGatherScatter)

#define ExtrAddKey4( lanes0, lanes1, lanes2, lanes3, p, argIndex )                                      \
                                    XOReq256(lanes0, CONST256_64(kRoll[argIndex+0])),                   \
                                    XOReq256(lanes1, CONST256_64(kRoll[argIndex+1])),                   \
                                    XOReq256(lanes2, CONST256_64(kRoll[argIndex+2])),                   \
                                    XOReq256(lanes3, CONST256_64(kRoll[argIndex+3])),                   \
                                    _mm256_i32scatter_epi64((long long int *)&p[argIndex+0], scatter, lanes0, 1),   \
                                    _mm256_i32scatter_epi64((long long int *)&p[argIndex+1], scatter, lanes1, 1),   \
                                    _mm256_i32scatter_epi64((long long int *)&p[argIndex+2], scatter, lanes2, 1),   \
                                    _mm256_i32scatter_epi64((long long int *)&p[argIndex+3], scatter, lanes3, 1)

#else

#define ExtrAddKey4( lanes0, lanes1, lanes2, lanes3, p, argIndex )                          \
                                    XOReq256(lanes0, CONST256_64(kRoll[argIndex+0])),       \
                                    XOReq256(lanes1, CONST256_64(kRoll[argIndex+1])),       \
                                    XOReq256(lanes2, CONST256_64(kRoll[argIndex+2])),       \
                                    XOReq256(lanes3, CONST256_64(kRoll[argIndex+3])),       \
                                    UNINTLEAVEa(lanes0, lanes1, lanes2, lanes3),            \
                                    STORE256u( p[argIndex+0*25], lanes0 ),                  \
                                    STORE256u( p[argIndex+1*25], lanes1 ),                  \
                                    STORE256u( p[argIndex+2*25], lanes2 ),                  \
                                    STORE256u( p[argIndex+3*25], lanes3 )

#endif

size_t KeccakP1600times4_AVX2_KravatteExpand(uint64_t *yAccu, const uint64_t *kRoll, unsigned char *output, size_t outputByteLen)
{
    uint64_t *out64 = (uint64_t *)output;
    size_t    nBlocks = outputByteLen / (4 * 200);
    declareABCDE
    #if    !defined(KeccakP1600times4_AVX2_fullUnrolling)
    unsigned int i;
    #endif
    V256    lanesL01, lanesL23, lanesH01, lanesH23;
    #if defined(UseGatherScatter)
    V128    scatter = _mm_setr_epi32(0*25*8, 1*25*8, 2*25*8, 3*25*8);
    #endif

    do {
        Aba = CONST256_64(yAccu[0]);
        Abe = CONST256_64(yAccu[1]);
        Abi = CONST256_64(yAccu[2]);
        Abo = CONST256_64(yAccu[3]);
        Abu = CONST256_64(yAccu[4]);

        Aga = CONST256_64(yAccu[5]);
        Age = CONST256_64(yAccu[6]);
        Agi = CONST256_64(yAccu[7]);
        Ago = CONST256_64(yAccu[8]);
        Agu = CONST256_64(yAccu[9]);

        Aka = CONST256_64(yAccu[10]);
        Ake = CONST256_64(yAccu[11]);
        Aki = CONST256_64(yAccu[12]);
        Ako = CONST256_64(yAccu[13]);
        Aku = CONST256_64(yAccu[14]);

        Ama = LOAD256u(yAccu[15]);
        Ame = LOAD256u(yAccu[16]);
        Ami = LOAD256u(yAccu[17]);
        Amo = LOAD256u(yAccu[18]);
        Amu = LOAD256u(yAccu[19]);

        ROL64in256(lanesL01, Ama, 7);
        ROL64in256(lanesH01, Ame, 18);
        lanesL01 = XOR256(lanesL01, lanesH01);
        lanesH01 = _mm256_and_si256(Ami, _mm256_srli_epi64(Ame, 1));
        lanesL01 = XOR256(lanesL01, lanesH01);

        Asa = LOAD256u(yAccu[20]);
        Ase = LOAD256u(yAccu[21]);
#if defined(__i386__) || defined(_M_IX86)
        Asi = _mm256_permute4x64_epi64(Ase, 0x39);
        Asi = _mm256_insert_epi32(Asi, _mm256_extract_epi32(lanesL01, 0), 6);
        Asi = _mm256_insert_epi32(Asi, _mm256_extract_epi32(lanesL01, 1), 7);
#else
        Asi = _mm256_insert_epi64(_mm256_permute4x64_epi64(Ase, 0x39), _mm256_extract_epi64(lanesL01, 0), 3);
#endif
        Aso = _mm256_permute2x128_si256(Ase, lanesL01, 0x21);
#if defined(__i386__) || defined(_M_IX86)
        Asu = _mm256_permute4x64_epi64(lanesL01, 0x93);
        Asu = _mm256_insert_epi32(Asu, _mm256_extract_epi32(Ase, 6), 0);
        Asu = _mm256_insert_epi32(Asu, _mm256_extract_epi32(Ase, 7), 1);
#else
        Asu = _mm256_insert_epi64(_mm256_permute4x64_epi64(lanesL01, 0x93), _mm256_extract_epi64(Ase, 3), 0);
#endif

        STORE256u(yAccu[15], Amu);
        yAccu[19] = _mm256_extract_epi64(Aso, 0);
        yAccu[20] = _mm256_extract_epi64(Aso, 1);
        STORE256u(yAccu[21], lanesL01);

        rounds6
        ExtrAddKey4(Aba, Abe, Abi, Abo, out64,  0 );
        ExtrAddKey4(Abu, Aga, Age, Agi, out64,  4 );
        ExtrAddKey4(Ago, Agu, Aka, Ake, out64,  8 );
        ExtrAddKey4(Aki, Ako, Aku, Ama, out64, 12 );
        ExtrAddKey4(Ame, Ami, Amo, Amu, out64, 16 );
        ExtrAddKey4(Asa, Ase, Asi, Aso, out64, 20 );
        ExtrAddKey( Asu,                out64, 24 );
        out64 += 4 * 25;
    }
    while(--nBlocks != 0);

    return (size_t)out64 - (size_t)output;
}

#undef OverWr4
#undef ExtrAddKey
#undef ExtrAddKey4

#undef Kravatte_Roll
#undef UNINTLEAVEa
#undef INTLEAVEa

#define initializeState(X) \
    X##ba = ZERO(); \
    X##be = ZERO(); \
    X##bi = ZERO(); \
    X##bo = ZERO(); \
    X##bu = ZERO(); \
    X##ga = ZERO(); \
    X##ge = ZERO(); \
    X##gi = ZERO(); \
    X##go = ZERO(); \
    X##gu = ZERO(); \
    X##ka = ZERO(); \
    X##ke = ZERO(); \
    X##ki = ZERO(); \
    X##ko = ZERO(); \
    X##ku = ZERO(); \
    X##ma = ZERO(); \
    X##me = ZERO(); \
    X##mi = ZERO(); \
    X##mo = ZERO(); \
    X##mu = ZERO(); \
    X##sa = ZERO(); \
    X##se = ZERO(); \
    X##si = ZERO(); \
    X##so = ZERO(); \
    X##su = ZERO(); \

#define XORdata4(X, data0, data1, data2, data3) \
    XOReq256(X##ba, LOAD4_64((data3)[ 0], (data2)[ 0], (data1)[ 0], (data0)[ 0])); \
    XOReq256(X##be, LOAD4_64((data3)[ 1], (data2)[ 1], (data1)[ 1], (data0)[ 1])); \
    XOReq256(X##bi, LOAD4_64((data3)[ 2], (data2)[ 2], (data1)[ 2], (data0)[ 2])); \
    XOReq256(X##bo, LOAD4_64((data3)[ 3], (data2)[ 3], (data1)[ 3], (data0)[ 3])); \

#define XORdata16(X, data0, data1, data2, data3) \
    XORdata4(X, data0, data1, data2, data3) \
    XOReq256(X##bu, LOAD4_64((data3)[ 4], (data2)[ 4], (data1)[ 4], (data0)[ 4])); \
    XOReq256(X##ga, LOAD4_64((data3)[ 5], (data2)[ 5], (data1)[ 5], (data0)[ 5])); \
    XOReq256(X##ge, LOAD4_64((data3)[ 6], (data2)[ 6], (data1)[ 6], (data0)[ 6])); \
    XOReq256(X##gi, LOAD4_64((data3)[ 7], (data2)[ 7], (data1)[ 7], (data0)[ 7])); \
    XOReq256(X##go, LOAD4_64((data3)[ 8], (data2)[ 8], (data1)[ 8], (data0)[ 8])); \
    XOReq256(X##gu, LOAD4_64((data3)[ 9], (data2)[ 9], (data1)[ 9], (data0)[ 9])); \
    XOReq256(X##ka, LOAD4_64((data3)[10], (data2)[10], (data1)[10], (data0)[10])); \
    XOReq256(X##ke, LOAD4_64((data3)[11], (data2)[11], (data1)[11], (data0)[11])); \
    XOReq256(X##ki, LOAD4_64((data3)[12], (data2)[12], (data1)[12], (data0)[12])); \
    XOReq256(X##ko, LOAD4_64((data3)[13], (data2)[13], (data1)[13], (data0)[13])); \
    XOReq256(X##ku, LOAD4_64((data3)[14], (data2)[14], (data1)[14], (data0)[14])); \
    XOReq256(X##ma, LOAD4_64((data3)[15], (data2)[15], (data1)[15], (data0)[15])); \

#define XORdata17(X, data0, data1, data2, data3) \
    XORdata16(X, data0, data1, data2, data3) \
    XOReq256(X##me, LOAD4_64((data3)[16], (data2)[16], (data1)[16], (data0)[16])); \

#define XORdata21(X, data0, data1, data2, data3) \
    XORdata17(X, data0, data1, data2, data3) \
    XOReq256(X##mi, LOAD4_64((data3)[17], (data2)[17], (data1)[17], (data0)[17])); \
    XOReq256(X##mo, LOAD4_64((data3)[18], (data2)[18], (data1)[18], (data0)[18])); \
    XOReq256(X##mu, LOAD4_64((data3)[19], (data2)[19], (data1)[19], (data0)[19])); \
    XOReq256(X##sa, LOAD4_64((data3)[20], (data2)[20], (data1)[20], (data0)[20])); \

#define rounds12 \
    prepareTheta \
    thetaRhoPiChiIotaPrepareTheta(12, A, E) \
    thetaRhoPiChiIotaPrepareTheta(13, E, A) \
    thetaRhoPiChiIotaPrepareTheta(14, A, E) \
    thetaRhoPiChiIotaPrepareTheta(15, E, A) \
    thetaRhoPiChiIotaPrepareTheta(16, A, E) \
    thetaRhoPiChiIotaPrepareTheta(17, E, A) \
    thetaRhoPiChiIotaPrepareTheta(18, A, E) \
    thetaRhoPiChiIotaPrepareTheta(19, E, A) \
    thetaRhoPiChiIotaPrepareTheta(20, A, E) \
    thetaRhoPiChiIotaPrepareTheta(21, E, A) \
    thetaRhoPiChiIotaPrepareTheta(22, A, E) \
    thetaRhoPiChiIota(23, E, A)

#define chunkSize 8192
#define KT128_rateInBytes (21*8)
#define KT256_rateInBytes (17*8)

void KeccakP1600times4_AVX2_KT128ProcessLeaves(const unsigned char *input, unsigned char *output)
{
    declareABCDE
    unsigned int j;

    initializeState(A);

    for(j = 0; j < (chunkSize - KT128_rateInBytes); j += KT128_rateInBytes) {
        XORdata21(A, (const uint64_t *)input, (const uint64_t *)(input+chunkSize), (const uint64_t *)(input+2*chunkSize), (const uint64_t *)(input+3*chunkSize));
        rounds12
        input += KT128_rateInBytes;
    }

    XORdata16(A, (const uint64_t *)input, (const uint64_t *)(input+chunkSize), (const uint64_t *)(input+2*chunkSize), (const uint64_t *)(input+3*chunkSize));
    XOReq256(Ame, CONST256_64(0x0BULL));
    XOReq256(Asa, CONST256_64(0x8000000000000000ULL));
    rounds12

    {
        __m256i lanesL01, lanesL23, lanesH01, lanesH23;

        lanesL01 = UNPACKL( Aba, Abe );
        lanesH01 = UNPACKH( Aba, Abe );
        lanesL23 = UNPACKL( Abi, Abo );
        lanesH23 = UNPACKH( Abi, Abo );
        STORE256u( output[ 0], PERM128( lanesL01, lanesL23, 0x20 ) );
        STORE256u( output[32], PERM128( lanesH01, lanesH23, 0x20 ) );
        STORE256u( output[64], PERM128( lanesL01, lanesL23, 0x31 ) );
        STORE256u( output[96], PERM128( lanesH01, lanesH23, 0x31 ) );
    }
}

void KeccakP1600times4_AVX2_KT256ProcessLeaves(const unsigned char *input, unsigned char *output)
{
    declareABCDE
    unsigned int j;

    initializeState(A);

    for(j = 0; j < (chunkSize - KT256_rateInBytes); j += KT256_rateInBytes) {
        XORdata17(A, (const uint64_t *)input, (const uint64_t *)(input+chunkSize), (const uint64_t *)(input+2*chunkSize), (const uint64_t *)(input+3*chunkSize));
        rounds12
        input += KT256_rateInBytes;
    }

    XORdata4(A, (const uint64_t *)input, (const uint64_t *)(input+chunkSize), (const uint64_t *)(input+2*chunkSize), (const uint64_t *)(input+3*chunkSize));
    XOReq256(Abu, CONST256_64(0x0BULL));
    XOReq256(Ame, CONST256_64(0x8000000000000000ULL));
    rounds12

    {
        __m256i lanesL01, lanesL23, lanesH01, lanesH23;

        lanesL01 = UNPACKL( Aba, Abe );
        lanesH01 = UNPACKH( Aba, Abe );
        lanesL23 = UNPACKL( Abi, Abo );
        lanesH23 = UNPACKH( Abi, Abo );
        STORE256u( output[  0], PERM128( lanesL01, lanesL23, 0x20 ) );
        STORE256u( output[ 64], PERM128( lanesH01, lanesH23, 0x20 ) );
        STORE256u( output[128], PERM128( lanesL01, lanesL23, 0x31 ) );
        STORE256u( output[192], PERM128( lanesH01, lanesH23, 0x31 ) );

        lanesL01 = UNPACKL( Abu, Aga );
        lanesH01 = UNPACKH( Abu, Aga );
        lanesL23 = UNPACKL( Age, Agi );
        lanesH23 = UNPACKH( Age, Agi );
        STORE256u( output[ 32], PERM128( lanesL01, lanesL23, 0x20 ) );
        STORE256u( output[ 96], PERM128( lanesH01, lanesH23, 0x20 ) );
        STORE256u( output[160], PERM128( lanesL01, lanesL23, 0x31 ) );
        STORE256u( output[224], PERM128( lanesH01, lanesH23, 0x31 ) );
    }
}
