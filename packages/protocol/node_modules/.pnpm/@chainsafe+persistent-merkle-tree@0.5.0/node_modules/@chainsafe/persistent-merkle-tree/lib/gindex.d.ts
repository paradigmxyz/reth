export declare type Gindex = bigint;
export declare type GindexBitstring = string;
export declare function bitIndexBigInt(v: bigint): number;
export declare function toGindex(depth: number, index: bigint): Gindex;
export declare function toGindexBitstring(depth: number, index: number): GindexBitstring;
export declare function convertGindexToBitstring(gindex: Gindex | GindexBitstring): GindexBitstring;
export declare function countToDepth(count: bigint): number;
/**
 * Iterate through Gindexes at a certain depth
 */
export declare function iterateAtDepth(depth: number, startIndex: bigint, count: bigint): Iterable<Gindex>;
/**
 * Return Gindexes at a certain depth
 */
export declare function getGindicesAtDepth(depth: number, startIndex: number, count: number): Gindex[];
export declare type Bit = 0 | 1;
export interface GindexIterator extends Iterable<Bit> {
    remainingBitLength(): number;
}
export declare function gindexIterator(gindex: Gindex | GindexBitstring): GindexIterator;
export declare function getGindexBits(gindex: Gindex | GindexBitstring): Bit[];
/**
 * Concatenate Generalized Indices
 * Given generalized indices i1 for A -> B, i2 for B -> C .... i_n for Y -> Z, returns
 * the generalized index for A -> Z.
 */
export declare function concatGindices(gindices: Gindex[]): Gindex;
export declare function gindexSibling(gindex: Gindex): Gindex;
export declare function gindexParent(gindex: Gindex): Gindex;
export declare function gindexChild(gindex: Gindex, rightChild: boolean): Gindex;
