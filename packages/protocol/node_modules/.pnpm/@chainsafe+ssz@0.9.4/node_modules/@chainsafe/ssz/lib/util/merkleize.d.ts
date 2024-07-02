export declare function hash64(bytes32A: Uint8Array, bytes32B: Uint8Array): Uint8Array;
export declare function merkleize(chunks: Uint8Array[], padFor: number): Uint8Array;
/**
 * Split a long Uint8Array into Uint8Array of exactly 32 bytes
 */
export declare function splitIntoRootChunks(longChunk: Uint8Array): Uint8Array[];
/** @ignore */
export declare function mixInLength(root: Uint8Array, length: number): Uint8Array;
export declare function bitLength(i: number): number;
/**
 * Given maxChunkCount return the chunkDepth
 * ```
 * n: [0,1,2,3,4,5,6,7,8,9]
 * d: [0,0,1,2,2,3,3,3,3,4]
 * ```
 */
export declare function maxChunksToDepth(n: number): number;
/** @ignore */
export declare function nextPowerOf2(n: number): number;
//# sourceMappingURL=merkleize.d.ts.map