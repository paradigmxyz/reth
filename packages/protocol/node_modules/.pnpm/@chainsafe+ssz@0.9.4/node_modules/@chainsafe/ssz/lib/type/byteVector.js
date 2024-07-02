"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ByteVectorType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const byteArray_1 = require("./byteArray");
/* eslint-disable @typescript-eslint/member-ordering */
/**
 * ByteVector: Immutable alias of Vector[byte, N]
 * - Notation: `ByteVector[N]`
 * - Value: `Uint8Array`
 * - View: `Uint8Array`
 * - ViewDU: `Uint8Array`
 *
 * ByteVector is an immutable value which is represented by a Uint8Array for memory efficiency and performance.
 * Note: Consumers of this type MUST never mutate the `Uint8Array` representation of a ByteVector.
 *
 * For a `ByteVectorType` with mutability, use `VectorBasicType(byteType)`
 */
class ByteVectorType extends byteArray_1.ByteArrayType {
    constructor(lengthBytes, opts) {
        super();
        this.lengthBytes = lengthBytes;
        this.isList = false;
        if (lengthBytes === 0)
            throw Error("Vector length must be > 0");
        this.typeName = opts?.typeName ?? `ByteVector[${lengthBytes}]`;
        this.maxChunkCount = Math.ceil(this.lengthBytes / 32);
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        this.depth = this.chunkDepth;
        this.fixedSize = this.lengthBytes;
        this.minSize = this.fixedSize;
        this.maxSize = this.fixedSize;
    }
    static named(limitBits, opts) {
        return new (named_1.namedClass(ByteVectorType, opts.typeName))(limitBits, opts);
    }
    // Views: inherited from ByteArrayType
    // Serialization + deserialization
    value_serializedSize() {
        return this.fixedSize;
    }
    // value_* inherited from ByteArrayType
    tree_serializedSize() {
        return this.fixedSize;
    }
    tree_serializeToBytes(output, offset, node) {
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(node, this.chunkDepth, 0, this.maxChunkCount);
        persistent_merkle_tree_1.packedNodeRootsToBytes(output.dataView, offset, this.fixedSize, nodes);
        return offset + this.fixedSize;
    }
    tree_deserializeFromBytes(data, start, end) {
        this.assertValidSize(end - start);
        return persistent_merkle_tree_1.packedRootsBytesToNode(this.chunkDepth, data.dataView, start, end);
    }
    tree_getByteLen() {
        return this.lengthBytes;
    }
    // Merkleization: inherited from ByteArrayType
    // Proofs: inherited from BitArrayType
    // JSON: inherited from ByteArrayType
    assertValidSize(size) {
        if (size !== this.lengthBytes) {
            throw Error(`ByteVector invalid size ${size} expected ${this.lengthBytes}`);
        }
    }
}
exports.ByteVectorType = ByteVectorType;
//# sourceMappingURL=byteVector.js.map