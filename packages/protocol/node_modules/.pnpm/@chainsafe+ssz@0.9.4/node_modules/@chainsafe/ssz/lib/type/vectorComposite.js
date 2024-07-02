"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.VectorCompositeType = void 0;
const merkleize_1 = require("../util/merkleize");
const named_1 = require("../util/named");
const arrayComposite_1 = require("./arrayComposite");
const arrayComposite_2 = require("../view/arrayComposite");
const arrayComposite_3 = require("../viewDU/arrayComposite");
const array_1 = require("./array");
/**
 * Vector: Ordered fixed-length homogeneous collection, with N values
 *
 * Array of Composite type:
 * - Composite types always take at least one chunk
 * - Composite types are always returned as views
 */
class VectorCompositeType extends array_1.ArrayType {
    constructor(elementType, length, opts) {
        super(elementType);
        this.elementType = elementType;
        this.length = length;
        this.itemsPerChunk = 1;
        this.isList = false;
        this.isViewMutable = true;
        if (elementType.isBasic)
            throw Error("elementType must not be basic");
        if (length === 0)
            throw Error("Vector length must be > 0");
        this.typeName = opts?.typeName ?? `Vector[${elementType.typeName}, ${length}]`;
        this.maxChunkCount = length;
        this.chunkDepth = merkleize_1.maxChunksToDepth(this.maxChunkCount);
        this.depth = this.chunkDepth;
        this.fixedSize = elementType.fixedSize === null ? null : length * elementType.fixedSize;
        this.minSize = arrayComposite_1.minSizeArrayComposite(elementType, length);
        this.maxSize = arrayComposite_1.maxSizeArrayComposite(elementType, length);
        this.defaultLen = length;
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    static named(elementType, limit, opts) {
        return new (named_1.namedClass(VectorCompositeType, opts.typeName))(elementType, limit, opts);
    }
    getView(tree) {
        return new arrayComposite_2.ArrayCompositeTreeView(this, tree);
    }
    getViewDU(node, cache) {
        // cache type should be validated (if applicate) in the view
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        return new arrayComposite_3.ArrayCompositeTreeViewDU(this, node, cache);
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
        return arrayComposite_1.value_serializedSizeArrayComposite(this.elementType, this.length, value);
    }
    value_serializeToBytes(output, offset, value) {
        return arrayComposite_1.value_serializeToBytesArrayComposite(this.elementType, this.length, output, offset, value);
    }
    value_deserializeFromBytes(data, start, end) {
        return arrayComposite_1.value_deserializeFromBytesArrayComposite(this.elementType, data, start, end, this);
    }
    tree_serializedSize(node) {
        return arrayComposite_1.tree_serializedSizeArrayComposite(this.elementType, this.length, this.depth, node);
    }
    tree_serializeToBytes(output, offset, node) {
        return arrayComposite_1.tree_serializeToBytesArrayComposite(this.elementType, this.length, this.depth, node, output, offset);
    }
    tree_deserializeFromBytes(data, start, end) {
        return arrayComposite_1.tree_deserializeFromBytesArrayComposite(this.elementType, this.depth, data, start, end, this);
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
        return arrayComposite_1.value_getRootsArrayComposite(this.elementType, this.length, value);
    }
}
exports.VectorCompositeType = VectorCompositeType;
//# sourceMappingURL=vectorComposite.js.map