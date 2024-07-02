"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BitVectorType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const bitArray_1 = require("../value/bitArray");
const bitArray_2 = require("./bitArray");
/**
 * BitVector: ordered fixed-length collection of boolean values, with N bits
 * - Notation: `Bitvector[N]`
 * - Value: `BitArray`, @see BitArray for a justification of its memory efficiency and performance
 * - View: `BitArrayTreeView`
 * - ViewDU: `BitArrayTreeViewDU`
 */
class BitVectorType extends bitArray_2.BitArrayType {
    constructor(lengthBits, opts) {
        super();
        this.lengthBits = lengthBits;
        this.isList = false;
        if (lengthBits === 0)
            throw Error("Vector length must be > 0");
        this.typeName = opts?.typeName ?? `BitVector[${lengthBits}]`;
        this.chunkCount = Math.ceil(this.lengthBits / 8 / 32);
        this.maxChunkCount = this.chunkCount;
        this.depth = merkleize_1.maxChunksToDepth(this.chunkCount);
        this.fixedSize = Math.ceil(this.lengthBits / 8);
        this.minSize = this.fixedSize;
        this.maxSize = this.fixedSize;
        // To cache mask for trailing zero bits validation
        this.zeroBitsMask = lengthBits % 8 === 0 ? 0 : 0xff & (0xff << lengthBits % 8);
    }
    static named(limitBits, opts) {
        return new (named_1.namedClass(BitVectorType, opts.typeName))(limitBits, opts);
    }
    defaultValue() {
        return bitArray_1.BitArray.fromBitLen(this.lengthBits);
    }
    // Views: inherited from BitArrayType
    // Serialization + deserialization
    value_serializedSize() {
        return this.fixedSize;
    }
    value_serializeToBytes(output, offset, value) {
        output.uint8Array.set(value.uint8Array, offset);
        return offset + this.fixedSize;
    }
    value_deserializeFromBytes(data, start, end) {
        this.assertValidLength(data.uint8Array, start, end);
        // Buffer.prototype.slice does not copy memory, Enforce Uint8Array usage https://github.com/nodejs/node/issues/28087
        return new bitArray_1.BitArray(Uint8Array.prototype.slice.call(data.uint8Array, start, end), this.lengthBits);
    }
    tree_serializedSize() {
        return this.fixedSize;
    }
    tree_serializeToBytes(output, offset, node) {
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, this.depth, 0, this.chunkCount);
        persistent_merkle_tree_1.packedNodeRootsToBytes(output.dataView, offset, this.fixedSize, nodes);
        return offset + this.fixedSize;
    }
    tree_deserializeFromBytes(data, start, end) {
        this.assertValidLength(data.uint8Array, start, end);
        return persistent_merkle_tree_1.packedRootsBytesToNode(this.depth, data.dataView, start, end);
    }
    tree_getByteLen() {
        return this.fixedSize;
    }
    // Merkleization: inherited from BitArrayType
    // Proofs: inherited from BitArrayType
    // JSON: inherited from BitArrayType
    // Deserializer helpers
    assertValidLength(data, start, end) {
        const size = end - start;
        if (end - start !== this.fixedSize) {
            throw Error(`Invalid BitVector size ${size} != ${this.fixedSize}`);
        }
        // If lengthBits is not aligned to bytes, ensure trailing bits are zeroed
        if (
        // If zeroBitsMask == 0, then the BitVector uses full bytes only
        this.zeroBitsMask > 0 &&
            // if the last byte is partial, retrieve it and use the cached mask to check if trailing bits are zeroed
            (data[end - 1] & this.zeroBitsMask) > 0) {
            throw Error("BitVector: nonzero bits past length");
        }
    }
}
exports.BitVectorType = BitVectorType;
//# sourceMappingURL=bitVector.js.map