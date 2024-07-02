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
export declare class BitArray {
    /** Underlying BitArray Uint8Array data */
    readonly uint8Array: Uint8Array;
    /** Immutable bitLen of this BitArray */
    readonly bitLen: number;
    constructor(
    /** Underlying BitArray Uint8Array data */
    uint8Array: Uint8Array, 
    /** Immutable bitLen of this BitArray */
    bitLen: number);
    /** Returns a zero'ed BitArray of `bitLen` */
    static fromBitLen(bitLen: number): BitArray;
    /** Returns a BitArray of `bitLen` with a single bit set to true at position `bitIndex` */
    static fromSingleBit(bitLen: number, bitIndex: number): BitArray;
    /** Returns a BitArray from an array of booleans representation */
    static fromBoolArray(bitBoolArr: boolean[]): BitArray;
    clone(): BitArray;
    /**
     * Get bit value at index `bitIndex`
     */
    get(bitIndex: number): boolean;
    /**
     * Set bit value at index `bitIndex`
     */
    set(bitIndex: number, bit: boolean): void;
    /** Merge two BitArray bitfields with OR. Must have the same bitLen */
    mergeOrWith(bitArray2: BitArray): void;
    /**
     * Returns an array with the indexes which have a bit set to true
     */
    intersectValues<T>(values: T[]): T[];
    /**
     * Returns the positions of all bits that are set to true
     */
    getTrueBitIndexes(): number[];
    /**
     * Return the position of a single bit set. If no bit set or more than 1 bit set, throws.
     * @returns
     *  - number: if there's a single bit set, the number it the single bit set position
     *  - null: if ERROR_MORE_THAN_ONE_BIT_SET or ERROR_NO_BIT_SET
     * @throws
     *  - ERROR_MORE_THAN_ONE_BIT_SET
     *  - ERROR_NO_BIT_SET
     */
    getSingleTrueBit(): number | null;
    toBoolArray(): boolean[];
}
/**
 * Given a byte (0 -> 255), return a Array of boolean with length = 8, big endian.
 * Ex: 1 => [true false false false false false false false]
 *     5 => [true false true false false fase false false]
 */
export declare function getUint8ByteToBitBooleanArray(byte: number): boolean[];
//# sourceMappingURL=bitArray.d.ts.map