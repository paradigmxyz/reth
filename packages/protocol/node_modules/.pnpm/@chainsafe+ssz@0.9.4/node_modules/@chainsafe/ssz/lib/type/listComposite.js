"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ListCompositeType = void 0;
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const arrayBasic_1 = require("./arrayBasic");
const arrayComposite_1 = require("./arrayComposite");
const listComposite_1 = require("../view/listComposite");
const listComposite_2 = require("../viewDU/listComposite");
const array_1 = require("./array");
/**
 * List: ordered variable-length homogeneous collection, limited to N values
 *
 * Array of Composite type:
 * - Composite types always take at least one chunk
 * - Composite types are always returned as views
 */
class ListCompositeType extends array_1.ArrayType {
    constructor(elementType, limit, opts) {
        super(elementType);
        this.elementType = elementType;
        this.limit = limit;
        this.itemsPerChunk = 1;
        this.fixedSize = null;
        this.isList = true;
        this.isViewMutable = true;
        this.defaultLen = 0;
        if (elementType.isBasic)
            throw Error("elementType must not be basic");
        if (limit === 0)
            throw Error("List limit must be > 0");
        this.typeName = opts?.typeName ?? `List[${elementType.typeName}, ${limit}]`;
        this.maxChunkCount = this.limit;
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        // Depth includes the extra level for the length node
        this.depth = this.chunkDepth + 1;
        this.minSize = 0;
        this.maxSize = arrayComposite_1.maxSizeArrayComposite(elementType, this.limit);
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    static named(elementType, limit, opts) {
        return new (named_1.namedClass(ListCompositeType, opts.typeName))(elementType, limit, opts);
    }
    getView(tree) {
        return new listComposite_1.ListCompositeTreeView(this, tree);
    }
    getViewDU(node, cache) {
        // cache type should be validated (if applicate) in the view
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        return new listComposite_2.ListCompositeTreeViewDU(this, node, cache);
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
        return arrayComposite_1.value_serializedSizeArrayComposite(this.elementType, value.length, value);
    }
    value_serializeToBytes(output, offset, value) {
        return arrayComposite_1.value_serializeToBytesArrayComposite(this.elementType, value.length, output, offset, value);
    }
    value_deserializeFromBytes(data, start, end) {
        return arrayComposite_1.value_deserializeFromBytesArrayComposite(this.elementType, data, start, end, this);
    }
    tree_serializedSize(node) {
        const chunksNode = this.tree_getChunksNode(node);
        const length = this.tree_getLength(node);
        return arrayComposite_1.tree_serializedSizeArrayComposite(this.elementType, length, this.chunkDepth, chunksNode);
    }
    tree_serializeToBytes(output, offset, node) {
        const chunksNode = this.tree_getChunksNode(node);
        const length = this.tree_getLength(node);
        return arrayComposite_1.tree_serializeToBytesArrayComposite(this.elementType, length, this.chunkDepth, chunksNode, output, offset);
    }
    tree_deserializeFromBytes(data, start, end) {
        return arrayComposite_1.tree_deserializeFromBytesArrayComposite(this.elementType, this.chunkDepth, data, start, end, this);
    }
    // Helpers for TreeView
    tree_getLength(node) {
        return arrayBasic_1.getLengthFromRootNode(node);
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
        return arrayComposite_1.value_getRootsArrayComposite(this.elementType, value.length, value);
    }
}
exports.ListCompositeType = ListCompositeType;
//# sourceMappingURL=listComposite.js.map