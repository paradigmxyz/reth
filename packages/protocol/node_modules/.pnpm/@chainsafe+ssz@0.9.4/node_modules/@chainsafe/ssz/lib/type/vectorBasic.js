"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.VectorBasicType = void 0;
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const arrayBasic_1 = require("./arrayBasic");
const arrayBasic_2 = require("../view/arrayBasic");
const arrayBasic_3 = require("../viewDU/arrayBasic");
const array_1 = require("./array");
/**
 * Vector: Ordered fixed-length homogeneous collection, with N values
 *
 * Array of Basic type:
 * - Basic types are max 32 bytes long so multiple values may be packed in the same node.
 * - Basic types are never returned in a view wrapper, but their value representation
 */
class VectorBasicType extends array_1.ArrayType {
    constructor(elementType, length, opts) {
        super(elementType);
        this.elementType = elementType;
        this.length = length;
        this.isList = false;
        this.isViewMutable = true;
        if (!elementType.isBasic)
            throw Error("elementType must be basic");
        if (length === 0)
            throw Error("Vector length must be > 0");
        this.typeName = opts?.typeName ?? `Vector[${elementType.typeName}, ${length}]`;
        // TODO Check that itemsPerChunk is an integer
        this.itemsPerChunk = 32 / elementType.byteLength;
        this.maxChunkCount = Math.ceil((length * elementType.byteLength) / 32);
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        this.depth = this.chunkDepth;
        this.fixedSize = length * elementType.byteLength;
        this.minSize = this.fixedSize;
        this.maxSize = this.fixedSize;
        this.defaultLen = length;
    }
    static named(elementType, limit, opts) {
        return new (named_1.namedClass(VectorBasicType, opts.typeName))(elementType, limit, opts);
    }
    getView(tree) {
        return new arrayBasic_2.ArrayBasicTreeView(this, tree);
    }
    getViewDU(node, cache) {
        // cache type should be validated (if applicate) in the view
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        return new arrayBasic_3.ArrayBasicTreeViewDU(this, node, cache);
    }
    commitView(view) {
        return view.node;
    }
    commitViewDU(view) {
        view.commit();
        return view.node;
    }
    cacheOfViewDU(view) {
        return view.cache;
    }
    // Serialization + deserialization
    value_serializedSize() {
        return this.fixedSize;
    }
    value_serializeToBytes(output, offset, value) {
        return arrayBasic_1.value_serializeToBytesArrayBasic(this.elementType, this.length, output, offset, value);
    }
    value_deserializeFromBytes(data, start, end) {
        return arrayBasic_1.value_deserializeFromBytesArrayBasic(this.elementType, data, start, end, this);
    }
    tree_serializedSize() {
        return this.fixedSize;
    }
    tree_serializeToBytes(output, offset, node) {
        return arrayBasic_1.tree_serializeToBytesArrayBasic(this.elementType, this.length, this.depth, output, offset, node);
    }
    tree_deserializeFromBytes(data, start, end) {
        return arrayBasic_1.tree_deserializeFromBytesArrayBasic(this.elementType, this.depth, data, start, end, this);
    }
    // Helpers for TreeView
    tree_getLength() {
        return this.length;
    }
    tree_setLength() {
        // Vector's length is immutable, ignore this call
    }
    tree_getChunksNode(node) {
        return node;
    }
    tree_setChunksNode(rootNode, chunksNode) {
        return chunksNode;
    }
    // Merkleization
    getRoots(value) {
        const uint8Array = new Uint8Array(this.fixedSize);
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        arrayBasic_1.value_serializeToBytesArrayBasic(this.elementType, this.length, { uint8Array, dataView }, 0, value);
        return merkleize_1.splitIntoRootChunks(uint8Array);
    }
}
exports.VectorBasicType = VectorBasicType;
//# sourceMappingURL=vectorBasic.js.map