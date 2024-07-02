"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getUint8ByteToBitBooleanArray = exports.BitArray = void 0;
/** Globally cache this information. @see getUint8ByteToBitBooleanArray */
const uint8ByteToBitBooleanArrays = new Array(256);
/**
 * BitArray may be represented as an array of bits or compressed into an array of bytes.
 *
 * **Array of bits**:
 * Require 8.87 bytes per bit, so for 512 bits = 4500 bytes.
 * Are 'faster' to iterate with native tooling but are as fast as array of bytes with precomputed caches.
 *
 * **Array of bytes**:
 * Require an average cost of Uint8Array in JS = 220 bytes for 32 bytes, so for 512 bits = 220 bytes.
 * With precomputed boolean arrays per bytes value are as fast to iterate as an array of bits above.
 *
 * This BitArray implementation will represent data as a Uint8Array since it's very cheap to deserialize and can be as
 * fast to iterate as a native array of booleans, precomputing boolean arrays (total memory cost of 16000 bytes).
 */
class BitArray {
    constructor(
    /** Underlying BitArray Uint8Array data */
    uint8Array, 
    /** Immutable bitLen of this BitArray */
    bitLen) {
        this.uint8Array = uint8Array;
        this.bitLen = bitLen;
        if (uint8Array.length !== Math.ceil(bitLen / 8)) {
            throw Error("BitArray uint8Array length does not match bitLen");
        }
    }
    /** Returns a zero'ed BitArray of `bitLen` */
    static fromBitLen(bitLen) {
        return new BitArray(new Uint8Array(Math.ceil(bitLen / 8)), bitLen);
    }
    /** Returns a BitArray of `bitLen` with a single bit set to true at position `bitIndex` */
    static fromSingleBit(bitLen, bitIndex) {
        const bitArray = BitArray.fromBitLen(bitLen);
        bitArray.set(bitIndex, true);
        return bitArray;
    }
    /** Returns a BitArray from an array of booleans representation */
    static fromBoolArray(bitBoolArr) {
        const bitArray = BitArray.fromBitLen(bitBoolArr.length);
        for (let i = 0; i < bitBoolArr.length; i++) {
            if (bitBoolArr[i] === true) {
                bitArray.set(i, true);
            }
        }
        return bitArray;
    }
    clone() {
        // TODO: Benchmark if Uint8Array.slice(0) is the fastest way to copy data here
        // Buffer.prototype.slice does not copy memory, Enforce Uint8Array usage https://github.com/nodejs/node/issues/28087
        return new BitArray(Uint8Array.prototype.slice.call(this.uint8Array, 0), this.bitLen);
    }
    /**
     * Get bit value at index `bitIndex`
     */
    get(bitIndex) {
        const byteIdx = Math.floor(bitIndex / 8);
        const bitInBit = bitIndex % 8;
        const mask = 1 << bitInBit;
        return (this.uint8Array[byteIdx] & mask) === mask;
    }
    /**
     * Set bit value at index `bitIndex`
     */
    set(bitIndex, bit) {
        if (bitIndex >= this.bitLen) {
            throw Error(`BitArray set bitIndex ${bitIndex} beyond bitLen ${this.bitLen}`);
        }
        const byteIdx = Math.floor(bitIndex / 8);
        const bitInBit = bitIndex % 8;
        const mask = 1 << bitInBit;
        let byte = this.uint8Array[byteIdx];
        if (bit) {
            // For bit in byte, 1,0 OR 1 = 1
            // byte 100110
            // mask 010000
            // res  110110
            byte |= mask;
            this.uint8Array[byteIdx] = byte;
        }
        else {
            // For bit in byte, 1,0 OR 1 = 0
            if ((byte & mask) === mask) {
                // byte 110110
                // mask 010000
                // res  100110
                byte ^= mask;
                this.uint8Array[byteIdx] = byte;
            }
            else {
                // Ok, bit is already 0
            }
        }
    }
    /** Merge two BitArray bitfields with OR. Must have the same bitLen */
    mergeOrWith(bitArray2) {
        if (bitArray2.bitLen !== this.bitLen) {
            throw Error("Must merge BitArrays of same bitLen");
        }
        // Merge bitFields
        for (let i = 0; i < this.uint8Array.length; i++) {
            this.uint8Array[i] = this.uint8Array[i] | bitArray2.uint8Array[i];
        }
    }
    /**
     * Returns an array with the indexes which have a bit set to true
     */
    intersectValues(values) {
        const yes = [];
        if (values.length !== this.bitLen) {
            throw Error(`Must not intersect values of length ${values.length} != bitLen ${this.bitLen}`);
        }
        const fullByteLen = Math.floor(this.bitLen / 8);
        const remainderBits = this.bitLen % 8;
        // Iterate over each byte of bits
        const bytes = this.uint8Array;
        for (let iByte = 0; iByte < fullByteLen; iByte++) {
            // Get the precomputed boolean array for this byte
            const booleansInByte = getUint8ByteToBitBooleanArray(bytes[iByte]);
            // For each bit in the byte check participation and add to indexesSelected array
            for (let iBit = 0; iBit < 8; iBit++) {
                if (booleansInByte[iBit]) {
                    yes.push(values[iByte * 8 + iBit]);
                }
            }
        }
        if (remainderBits > 0) {
            // Get the precomputed boolean array for this byte
            const booleansInByte = getUint8ByteToBitBooleanArray(bytes[fullByteLen]);
            // For each bit in the byte check participation and add to indexesSelected array
            for (let iBit = 0; iBit < remainderBits; iBit++) {
                if (booleansInByte[iBit]) {
                    yes.push(values[fullByteLen * 8 + iBit]);
                }
            }
        }
        return yes;
    }
    /**
     * Returns the positions of all bits that are set to true
     */
    getTrueBitIndexes() {
        const indexes = [];
        // Iterate over each byte of bits
        const bytes = this.uint8Array;
        for (let iByte = 0, byteLen = bytes.length; iByte < byteLen; iByte++) {
            // Get the precomputed boolean array for this byte
            const booleansInByte = getUint8ByteToBitBooleanArray(bytes[iByte]);
            // For each bit in the byte check participation and add to indexesSelected array
            for (let iBit = 0; iBit < 8; iBit++) {
                if (booleansInByte[iBit]) {
                    indexes.push(iByte * 8 + iBit);
                }
            }
        }
        return indexes;
    }
    /**
     * Return the position of a single bit set. If no bit set or more than 1 bit set, throws.
     * @returns
     *  - number: if there's a single bit set, the number it the single bit set position
     *  - null: if ERROR_MORE_THAN_ONE_BIT_SET or ERROR_NO_BIT_SET
     * @throws
     *  - ERROR_MORE_THAN_ONE_BIT_SET
     *  - ERROR_NO_BIT_SET
     */
    getSingleTrueBit() {
        let index = null;
        const bytes = this.uint8Array;
        // Iterate over each byte of bits
        for (let iByte = 0, byteLen = bytes.length; iByte < byteLen; iByte++) {
            // If it's exactly zero, there won't be any indexes, continue early
            if (bytes[iByte] === 0) {
                continue;
            }
            // Get the precomputed boolean array for this byte
            const booleansInByte = getUint8ByteToBitBooleanArray(bytes[iByte]);
            // For each bit in the byte check participation and add to indexesSelected array
            for (let iBit = 0; iBit < 8; iBit++) {
                if (booleansInByte[iBit] === true) {
                    if (index !== null) {
                        // ERROR_MORE_THAN_ONE_BIT_SET
                        return null;
                    }
                    index = iByte * 8 + iBit;
                }
            }
        }
        if (index === null) {
            // ERROR_NO_BIT_SET
            return null;
        }
        else {
            return index;
        }
    }
    toBoolArray() {
        const bitBoolArr = new Array(this.bitLen);
        for (let i = 0; i < this.bitLen; i++) {
            bitBoolArr[i] = this.get(i);
        }
        return bitBoolArr;
    }
}
exports.BitArray = BitArray;
/**
 * Given a byte (0 -> 255), return a Array of boolean with length = 8, big endian.
 * Ex: 1 => [true false false false false false false false]
 *     5 => [true false true false false fase false false]
 */
function getUint8ByteToBitBooleanArray(byte) {
    if (!uint8ByteToBitBooleanArrays[byte]) {
        uint8ByteToBitBooleanArrays[byte] = computeUint8ByteToBitBooleanArray(byte);
    }
    return uint8ByteToBitBooleanArrays[byte];
}
exports.getUint8ByteToBitBooleanArray = getUint8ByteToBitBooleanArray;
/** @see getUint8ByteToBitBooleanArray */
function computeUint8ByteToBitBooleanArray(byte) {
    // this returns little endian
    const binaryStr = byte.toString(2);
    const binaryLength = binaryStr.length;
    const bits = new Array(8);
    for (let i = 0; i < 8; i++) {
        bits[i] =
            i < binaryLength
                ? //
                    binaryStr[binaryLength - i - 1] === "1"
                : false;
    }
    return bits;
}
//# sourceMappingURL=bitArray.js.map