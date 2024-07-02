"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ListBasicType = void 0;
const arrayBasic_1 = require("./arrayBasic");
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const listBasic_1 = require("../view/listBasic");
const listBasic_2 = require("../viewDU/listBasic");
const array_1 = require("./array");
/**
 * List: ordered variable-length homogeneous collection, limited to N values
 *
 * Array of Basic type:
 * - Basic types are max 32 bytes long so multiple values may be packed in the same node.
 * - Basic types are never returned in a view wrapper, but their value representation
 */
class ListBasicType extends array_1.ArrayType {
    constructor(elementType, limit, opts) {
        super(elementType);
        this.elementType = elementType;
        this.limit = limit;
        this.fixedSize = null;
        this.isList = true;
        this.isViewMutable = true;
        this.defaultLen = 0;
        if (!elementType.isBasic)
            throw Error("elementType must be basic");
        if (limit === 0)
            throw Error("List limit must be > 0");
        this.typeName = opts?.typeName ?? `List[${elementType.typeName}, ${limit}]`;
        // TODO Check that itemsPerChunk is an integer
        this.itemsPerChunk = 32 / elementType.byteLength;
        this.maxChunkCount = Math.ceil((this.limit * elementType.byteLength) / 32);
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        // Depth includes the extra level for the length node
        this.depth = this.chunkDepth + 1;
        this.minSize = 0;
        this.maxSize = this.limit * elementType.maxSize;
    }
    static named(elementType, limit, opts) {
        return new (named_1.namedClass(ListBasicType, opts.typeName))(elementType, limit, opts);
    }
    getView(tree) {
        return new listBasic_1.ListBasicTreeView(this, tree);
    }
    getViewDU(node, cache) {
        // cache type should be validated (if applicate) in the view
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        return new listBasic_2.ListBasicTreeViewDU(this, node, cache);
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
    value_serializedSize(value) {
        return value.length * this.elementType.byteLength;
    }
    value_serializeToBytes(output, offset, value) {
        return arrayBasic_1.value_serializeToBytesArrayBasic(this.elementType, value.length, output, offset, value);
    }
    value_deserializeFromBytes(data, start, end) {
        return arrayBasic_1.value_deserializeFromBytesArrayBasic(this.elementType, data, start, end, this);
    }
    tree_serializedSize(node) {
        return this.tree_getLength(node) * this.elementType.byteLength;
    }
    tree_serializeToBytes(output, offset, node) {
        const chunksNode = this.tree_getChunksNode(node);
        const length = this.tree_getLength(node);
        return arrayBasic_1.tree_serializeToBytesArrayBasic(this.elementType, length, this.chunkDepth, output, offset, chunksNode);
    }
    tree_deserializeFromBytes(data, start, end) {
        return arrayBasic_1.tree_deserializeFromBytesArrayBasic(this.elementType, this.chunkDepth, data, start, end, this);
    }
    // Helpers for TreeView
    tree_getLength(node) {
        return node.right.getUint(4, 0);
    }
    tree_setLength(tree, length) {
        tree.rootNode = arrayBasic_1.addLengthNode(tree.rootNode.left, length);
    }
    tree_getChunksNode(node) {
        return node.left;
    }
    tree_setChunksNode(rootNode, chunksNode, newLength) {
        return arrayBasic_1.setChunksNode(rootNode, chunksNode, newLength);
    }
    // Merkleization
    hashTreeRoot(value) {
        return merkleize_1.mixInLength(super.hashTreeRoot(value), value.length);
    }
    getRoots(value) {
        const uint8Array = new Uint8Array(this.value_serializedSize(value));
        const dataView = new DataView(uint8Array.buffer, uint8Array.byteOffset, uint8Array.byteLength);
        arrayBasic_1.value_serializeToBytesArrayBasic(this.elementType, value.length, { uint8Array, dataView }, 0, value);
        return merkleize_1.splitIntoRootChunks(uint8Array);
    }
}
exports.ListBasicType = ListBasicType;
//# sourceMappingURL=listBasic.js.map