"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ByteListType = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const arrayBasic_1 = require("./arrayBasic");
const byteArray_1 = require("./byteArray");
/**
 * ByteList: Immutable alias of List[byte, N]
 * - Notation: `ByteList[N]`
 * - Value: `Uint8Array`
 * - View: `Uint8Array`
 * - ViewDU: `Uint8Array`
 *
 * ByteList is an immutable value which is represented by a Uint8Array for memory efficiency and performance.
 * Note: Consumers of this type MUST never mutate the `Uint8Array` representation of a ByteList.
 *
 * For a `ByteListType` with mutability, use `ListBasicType(byteType)`
 */
class ByteListType extends byteArray_1.ByteArrayType {
    constructor(limitBytes, opts) {
        super();
        this.limitBytes = limitBytes;
        this.fixedSize = null;
        this.isList = true;
        if (limitBytes === 0)
            throw Error("List limit must be > 0");
        this.typeName = opts?.typeName ?? `ByteList[${limitBytes}]`;
        this.maxChunkCount = Math.ceil(this.limitBytes / 32);
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        this.depth = 1 + this.chunkDepth;
        this.minSize = 0;
        this.maxSize = this.limitBytes;
    }
    static named(limitBits, opts) {
        return new (named_1.namedClass(ByteListType, opts.typeName))(limitBits, opts);
    }
    // Views: inherited from ByteArrayType
    // Serialization + deserialization
    value_serializedSize(value) {
        return value.length;
    }
    // value_* inherited from ByteArrayType
    tree_serializedSize(node) {
        return arrayBasic_1.getLengthFromRootNode(node);
    }
    tree_serializeToBytes(output, offset, node) {
        const chunksNode = arrayBasic_1.getChunksNodeFromRootNode(node);
        const byteLen = arrayBasic_1.getLengthFromRootNode(node);
        const chunkLen = Math.ceil(byteLen / 32);
        const nodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.chunkDepth, 0, chunkLen);
        persistent_merkle_tree_1.packedNodeRootsToBytes(output.dataView, offset, byteLen, nodes);
        return offset + byteLen;
    }
    tree_deserializeFromBytes(data, start, end) {
        this.assertValidSize(end - start);
        const chunksNode = persistent_merkle_tree_1.packedRootsBytesToNode(this.chunkDepth, data.dataView, start, end);
        return arrayBasic_1.addLengthNode(chunksNode, end - start);
    }
    tree_getByteLen(node) {
        if (!node)
            throw new Error("ByteListType requires a node to get leaves");
        return arrayBasic_1.getLengthFromRootNode(node);
    }
    // Merkleization: inherited from ByteArrayType
    hashTreeRoot(value) {
        return merkleize_1.mixInLength(super.hashTreeRoot(value), value.length);
    }
    // Proofs: inherited from BitArrayType
    // JSON: inherited from ByteArrayType
    assertValidSize(size) {
        if (size > this.limitBytes) {
            throw Error(`ByteList invalid size ${size} limit ${this.limitBytes}`);
        }
    }
}
exports.ByteListType = ByteListType;
//# sourceMappingURL=byteList.js.map