"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.nextPowerOf2 = exports.maxChunksToDepth = exports.bitLength = exports.mixInLength = exports.splitIntoRootChunks = exports.merkleize = exports.hash64 = void 0;
const as_sha256_1 = require("@chainsafe/as-sha256");
const zeros_1 = require("./zeros");
function hash64(bytes32A, bytes32B) {
    return as_sha256_1.digest2Bytes32(bytes32A, bytes32B);
}
exports.hash64 = hash64;
function merkleize(chunks, padFor) {
    const layerCount = bitLength(nextPowerOf2(padFor) - 1);
    if (chunks.length == 0) {
        return zeros_1.zeroHash(layerCount);
    }
    let chunkCount = chunks.length;
    // Instead of pushing on all padding zero chunks at the leaf level
    // we push on zero hash chunks at the highest possible level to avoid over-hashing
    for (let l = 0; l < layerCount; l++) {
        const padCount = chunkCount % 2;
        const paddedChunkCount = chunkCount + padCount;
        // if the chunks.length is odd
        // we need to push on the zero-hash of that level to merkleize that level
        for (let i = 0; i < padCount; i++) {
            chunks[chunkCount + i] = zeros_1.zeroHash(l);
        }
        for (let i = 0; i < paddedChunkCount; i += 2) {
            chunks[i / 2] = hash64(chunks[i], chunks[i + 1]);
        }
        chunkCount = paddedChunkCount / 2;
    }
    return chunks[0];
}
exports.merkleize = merkleize;
/**
 * Split a long Uint8Array into Uint8Array of exactly 32 bytes
 */
function splitIntoRootChunks(longChunk) {
    const chunkCount = Math.ceil(longChunk.length / 32);
    const chunks = new Array(chunkCount);
    for (let i = 0; i < chunkCount; i++) {
        const chunk = new Uint8Array(32);
        chunk.set(longChunk.slice(i * 32, (i + 1) * 32));
        chunks[i] = chunk;
    }
    return chunks;
}
exports.splitIntoRootChunks = splitIntoRootChunks;
/** @ignore */
function mixInLength(root, length) {
    const lengthBuf = Buffer.alloc(32);
    lengthBuf.writeUIntLE(length, 0, 6);
    return hash64(root, lengthBuf);
}
exports.mixInLength = mixInLength;
// x2 faster than bitLengthStr() which uses Number.toString(2)
function bitLength(i) {
    if (i === 0) {
        return 0;
    }
    return Math.floor(Math.log2(i)) + 1;
}
exports.bitLength = bitLength;
/**
 * Given maxChunkCount return the chunkDepth
 * ```
 * n: [0,1,2,3,4,5,6,7,8,9]
 * d: [0,0,1,2,2,3,3,3,3,4]
 * ```
 */
function maxChunksToDepth(n) {
    if (n === 0)
        return 0;
    return Math.ceil(Math.log2(n));
}
exports.maxChunksToDepth = maxChunksToDepth;
/** @ignore */
function nextPowerOf2(n) {
    return n <= 0 ? 1 : Math.pow(2, bitLength(n - 1));
}
exports.nextPowerOf2 = nextPowerOf2;
//# sourceMappingURL=merkleize.js.map