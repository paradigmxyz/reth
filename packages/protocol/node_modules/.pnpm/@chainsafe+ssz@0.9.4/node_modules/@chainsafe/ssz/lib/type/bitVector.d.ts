import { Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./composite";
import { BitArray } from "../value/bitArray";
import { BitArrayType } from "./bitArray";
export interface BitVectorOptions {
    typeName?: string;
}
/**
 * BitVector: ordered fixed-length collection of boolean values, with N bits
 * - Notation: `Bitvector[N]`
 * - Value: `BitArray`, @see BitArray for a justification of its memory efficiency and performance
 * - View: `BitArrayTreeView`
 * - ViewDU: `BitArrayTreeViewDU`
 */
export declare class BitVectorType extends BitArrayType {
    readonly lengthBits: number;
    readonly typeName: string;
    readonly chunkCount: number;
    readonly depth: number;
    readonly fixedSize: number;
    readonly minSize: number;
    readonly maxSize: number;
    readonly maxChunkCount: number;
    readonly isList = false;
    /**
     * Mask to check if trailing bits are zero'ed. Mask returns bits that must be zero'ed
     * ```
     * lengthBits % 8 | zeroBitsMask
     * 0              | 0
     * 1              | 11111110
     * 2              | 11111100
     * 7              | 10000000
     * ```
     */
    private readonly zeroBitsMask;
    constructor(lengthBits: number, opts?: BitVectorOptions);
    static named(limitBits: number, opts: Require<BitVectorOptions, "typeName">): BitVectorType;
    defaultValue(): BitArray;
    value_serializedSize(): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: BitArray): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): BitArray;
    tree_serializedSize(): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getByteLen(): number;
    private assertValidLength;
}
//# sourceMappingURL=bitVector.d.ts.map