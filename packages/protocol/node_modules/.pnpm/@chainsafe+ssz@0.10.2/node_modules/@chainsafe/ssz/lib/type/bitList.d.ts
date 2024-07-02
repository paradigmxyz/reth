import { Node } from "@chainsafe/persistent-merkle-tree";
import { Require } from "../util/types";
import { ByteViews } from "./composite";
import { BitArray } from "../value/bitArray";
import { BitArrayType } from "./bitArray";
export interface BitListOptions {
    typeName?: string;
}
/**
 * BitList: ordered variable-length collection of boolean values, limited to N bits
 * - Notation `Bitlist[N]`
 * - Value: `BitArray`, @see BitArray for a justification of its memory efficiency and performance
 * - View: `BitArrayTreeView`
 * - ViewDU: `BitArrayTreeViewDU`
 */
export declare class BitListType extends BitArrayType {
    readonly limitBits: number;
    readonly typeName: string;
    readonly depth: number;
    readonly chunkDepth: number;
    readonly fixedSize: null;
    readonly minSize = 1;
    readonly maxSize: number;
    readonly maxChunkCount: number;
    readonly isList = true;
    constructor(limitBits: number, opts?: BitListOptions);
    static named(limitBits: number, opts: Require<BitListOptions, "typeName">): BitListType;
    defaultValue(): BitArray;
    value_serializedSize(value: BitArray): number;
    value_serializeToBytes(output: ByteViews, offset: number, value: BitArray): number;
    value_deserializeFromBytes(data: ByteViews, start: number, end: number): BitArray;
    tree_serializedSize(node: Node): number;
    tree_serializeToBytes(output: ByteViews, offset: number, node: Node): number;
    tree_deserializeFromBytes(data: ByteViews, start: number, end: number): Node;
    tree_getByteLen(node?: Node): number;
    hashTreeRoot(value: BitArray): Uint8Array;
    private deserializeUint8ArrayBitListFromBytes;
}
//# sourceMappingURL=bitList.d.ts.map