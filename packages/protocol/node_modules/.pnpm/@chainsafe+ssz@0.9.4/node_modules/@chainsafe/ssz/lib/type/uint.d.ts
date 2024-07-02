import { LeafNode, Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./abstract";
import { BasicType } from "./basic";
export interface UintNumberOpts {
    /** Represent the value 2^64-1 as the symbolic value `+Infinity`. @see UintNumberType for a justification. */
    clipInfinity?: boolean;
    /** For `tree_setToPackedNode` set values with bitwise OR instead of a regular set */
    setBitwiseOR?: boolean;
    typeName?: string;
}
export declare type UintNumberByteLen = 1 | 2 | 4 | 8;
export declare type UintBigintByteLen = 1 | 2 | 4 | 8 | 16 | 32;
export declare const uintNumberByteLens: UintNumberByteLen[];
export declare const uintBigintByteLens: UintBigintByteLen[];
/**
 * Uint: N-bit unsigned integer (where N in [8, 16, 32, 64, 128, 256])
 * - Notation: uintN
 *
 * UintNumber is represented as the Javascript primitive value 'Number'.
 *
 * The Number type is a double-precision 64-bit binary format IEEE 754 value (numbers between -(2^53 − 1) and
 * 2^53 − 1). It also has the symbolic value: +Infinity.
 *
 * As of 2021 performance of 'Number' is extremely faster than 'BigInt'. Some values are spec'ed as Uint64 but
 * practically they will never exceed 53 bits, such as any unit time or simple counters. This type is an optimization
 * for these cases, as UintNumber64 can represent any value between 0 and 2^53−1 as well as the max value 2^64-1.
 */
export declare class UintNumberType extends BasicType<number> {
    readonly byteLength: UintNumberByteLen;
    readonly typeName: string;
    readonly itemsPerChunk: number;
    readonly fixedSize: number;
    readonly minSize: number;
    readonly maxSize: number;
    private readonly maxDecimalStr;
    private readonly clipInfinity;
    private readonly setBitwiseOR;
    constructor(byteLength: UintNumberByteLen, opts?: UintNumberOpts);
    static named(byteLength: UintNumberByteLen, opts: Require<UintNumberOpts, "typeName">): UintNumberType;
    defaultValue(): number;
    value_serializeToBytes({ dataView }: ByteViews, offset: number, value: number): number;
    value_deserializeFromBytes({ dataView }: ByteViews, start: number, end: number): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getFromNode(leafNode: LeafNode): number;
    tree_setToNode(leafNode: LeafNode, value: number): void;
    tree_getFromPackedNode(leafNode: LeafNode, index: number): number;
    tree_setToPackedNode(leafNode: LeafNode, index: number, value: number): void;
    fromJson(json: unknown): number;
    toJson(value: number): unknown;
}
export interface UintBigintOpts {
    typeName?: string;
}
/**
 * Uint: N-bit unsigned integer (where N in [8, 16, 32, 64, 128, 256])
 * - Notation: uintN
 *
 * UintBigint is represented as the Javascript primitive value 'BigInt'.
 *
 * The BigInt type is a numeric primitive in JavaScript that can represent integers with arbitrary precision.
 * With BigInts, you can safely store and operate on large integers even beyond the safe integer limit for Numbers.
 *
 * As of 2021 performance of 'Number' is extremely faster than 'BigInt'. For Uint values under 53 bits use UintNumber.
 * For other values that may exceed 53 bits, use UintBigint.
 */
export declare class UintBigintType extends BasicType<bigint> {
    readonly byteLength: UintBigintByteLen;
    readonly typeName: string;
    readonly itemsPerChunk: number;
    readonly fixedSize: number;
    readonly minSize: number;
    readonly maxSize: number;
    constructor(byteLength: UintBigintByteLen, opts?: UintBigintOpts);
    static named(byteLength: UintBigintByteLen, opts: Require<UintBigintOpts, "typeName">): UintBigintType;
    defaultValue(): bigint;
    value_serializeToBytes({ dataView }: ByteViews, offset: number, value: bigint): number;
    value_deserializeFromBytes({ dataView }: ByteViews, start: number, end: number): bigint;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getFromNode(leafNode: LeafNode): bigint;
    /** Mutates node to set value */
    tree_setToNode(leafNode: LeafNode, value: bigint): void;
    /** EXAMPLE of `tree_getFromNode` */
    tree_getFromPackedNode(leafNode: LeafNode, index: number): bigint;
    /** Mutates node to set value */
    tree_setToPackedNode(leafNode: LeafNode, index: number, value: bigint): void;
    fromJson(json: unknown): bigint;
    toJson(value: bigint): unknown;
}
//# sourceMappingURL=uint.d.ts.map