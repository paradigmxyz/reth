import { Tree, Node } from "@chainsafe/persistent-merkle-tree";
import { BitArray } from "../value/bitArray";
import { CompositeType } from "../type/composite";
import { TreeView } from "./abstract";
/**
 * Thin wrapper around BitArray to upstream changes to `tree` on every `this.set()`
 */
export declare class BitArrayTreeView extends TreeView<CompositeType<BitArray, unknown, unknown>> implements BitArray {
    readonly type: CompositeType<BitArray, unknown, unknown>;
    protected tree: Tree;
    private readonly bitArray;
    constructor(type: CompositeType<BitArray, unknown, unknown>, tree: Tree);
    get node(): Node;
    /** @see BitArray.uint8Array */
    get uint8Array(): Uint8Array;
    /** @see BitArray.bitLen */
    get bitLen(): number;
    /** @see BitArray.get */
    get(bitIndex: number): boolean;
    /** @see BitArray.set */
    set(bitIndex: number, bit: boolean): void;
    /** @see BitArray.mergeOrWith */
    mergeOrWith(bitArray2: BitArray): void;
    /** @see BitArray.intersectValues */
    intersectValues<T>(values: T[]): T[];
    /** @see BitArray.getTrueBitIndexes */
    getTrueBitIndexes(): number[];
    /** @see BitArray.getSingleTrueBit */
    getSingleTrueBit(): number | null;
    /** @see BitArray.toBoolArray */
    toBoolArray(): boolean[];
}
//# sourceMappingURL=bitArray.d.ts.map