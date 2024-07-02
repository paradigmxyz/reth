import { Node } from "@chainsafe/persistent-merkle-tree";
import { BitArray } from "../value/bitArray";
import { CompositeType } from "../type/composite";
import { TreeViewDU } from "./abstract";
/**
 * Thin wrapper around BitArray to upstream changes after `this.commit()`
 */
export declare class BitArrayTreeViewDU extends TreeViewDU<CompositeType<BitArray, unknown, unknown>> implements BitArray {
    readonly type: CompositeType<BitArray, unknown, unknown>;
    protected _rootNode: Node;
    /** Cached BitArray instance computed only on demand */
    private _bitArray;
    constructor(type: CompositeType<BitArray, unknown, unknown>, _rootNode: Node);
    get node(): Node;
    get cache(): unknown;
    commit(): void;
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
    /** Lazily computed bitArray instance */
    private get bitArray();
    protected clearCache(): void;
}
//# sourceMappingURL=bitArray.d.ts.map