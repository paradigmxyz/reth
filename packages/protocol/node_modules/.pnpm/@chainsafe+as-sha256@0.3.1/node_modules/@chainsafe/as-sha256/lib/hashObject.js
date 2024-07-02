"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.byteArrayToHashObject = exports.hashObjectToByteArray = void 0;
/**
 * Pass 8 numbers in an object and set that to inputArray.
 * This function contains multiple same procedures but we intentionally
 * do it step by step to improve performance a bit.
 **/
function hashObjectToByteArray(obj, byteArr, offset) {
    let tmp = obj.h0;
    byteArr[0 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[1 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[2 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[3 + offset] = tmp & 0xff;
    tmp = obj.h1;
    byteArr[4 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[5 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[6 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[7 + offset] = tmp & 0xff;
    tmp = obj.h2;
    byteArr[8 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[9 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[10 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[11 + offset] = tmp & 0xff;
    tmp = obj.h3;
    byteArr[12 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[13 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[14 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[15 + offset] = tmp & 0xff;
    tmp = obj.h4;
    byteArr[16 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[17 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[18 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[19 + offset] = tmp & 0xff;
    tmp = obj.h5;
    byteArr[20 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[21 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[22 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[23 + offset] = tmp & 0xff;
    tmp = obj.h6;
    byteArr[24 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[25 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[26 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[27 + offset] = tmp & 0xff;
    tmp = obj.h7;
    byteArr[28 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[29 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[30 + offset] = tmp & 0xff;
    tmp = tmp >> 8;
    byteArr[31 + offset] = tmp & 0xff;
}
exports.hashObjectToByteArray = hashObjectToByteArray;
/**
 * Parse outputArray into an object of 8 numbers.
 * This is the order that makes Uint32Array the same to Uint8Array
 * This function contains multiple same procedures but we intentionally
 * do it step by step to improve performance a bit.
 **/
function byteArrayToHashObject(byteArr) {
    let tmp = 0;
    tmp |= byteArr[3] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[2] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[1] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[0] & 0xff;
    const h0 = tmp;
    tmp = 0;
    tmp |= byteArr[7] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[6] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[5] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[4] & 0xff;
    const h1 = tmp;
    tmp = 0;
    tmp |= byteArr[11] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[10] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[9] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[8] & 0xff;
    const h2 = tmp;
    tmp = 0;
    tmp |= byteArr[15] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[14] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[13] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[12] & 0xff;
    const h3 = tmp;
    tmp = 0;
    tmp |= byteArr[19] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[18] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[17] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[16] & 0xff;
    const h4 = tmp;
    tmp = 0;
    tmp |= byteArr[23] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[22] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[21] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[20] & 0xff;
    const h5 = tmp;
    tmp = 0;
    tmp |= byteArr[27] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[26] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[25] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[24] & 0xff;
    const h6 = tmp;
    tmp = 0;
    tmp |= byteArr[31] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[30] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[29] & 0xff;
    tmp = tmp << 8;
    tmp |= byteArr[28] & 0xff;
    const h7 = tmp;
    return {
        h0,
        h1,
        h2,
        h3,
        h4,
        h5,
        h6,
        h7,
    };
}
exports.byteArrayToHashObject = byteArrayToHashObject;
//# sourceMappingURL=hashObject.js.map