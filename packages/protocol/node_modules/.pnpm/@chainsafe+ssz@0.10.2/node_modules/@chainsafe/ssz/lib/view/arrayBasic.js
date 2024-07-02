"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArrayBasicTreeView = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const abstract_1 = require("./abstract");
class ArrayBasicTreeView extends abstract_1.TreeView {
    constructor(type, tree) {
        super();
        this.type = type;
        this.tree = tree;
    }
    /**
     * Number of elements in the array. Equal to the Uint32 value of the Tree's length node
     */
    get length() {
        return this.type.tree_getLength(this.tree.rootNode);
    }
    get node() {
        return this.tree.rootNode;
    }
    /**
     * Get element at `index`. Returns the Basic element type value directly
     */
    get(index) {
        // First walk through the tree to get the root node for that index
        const chunkIndex = Math.floor(index / this.type.itemsPerChunk);
        const leafNode = this.tree.getNodeAtDepth(this.type.depth, chunkIndex);
        // eslint-disable-next-line @typescript-eslint/no-unsafe-return
        return this.type.elementType.tree_getFromPackedNode(leafNode, index);
    }
    /**
     * Set Basic element type `value` at `index`
     */
    set(index, value) {
        const length = this.length;
        if (index >= length) {
            throw Error(`Error setting index over length ${index} > ${length}`);
        }
        const chunkIndex = Math.floor(index / this.type.itemsPerChunk);
        const leafNodePrev = this.tree.getNodeAtDepth(this.type.depth, chunkIndex);
        // Create a new node to preserve immutability
        const leafNode = leafNodePrev.clone();
        this.type.elementType.tree_setToPackedNode(leafNode, index, value);
        // Commit immediately
        this.tree.setNodeAtDepth(this.type.depth, chunkIndex, leafNode);
    }
    /**
     * Get all values of this array as Basic element type values, from index zero to `this.length - 1`
     */
    getAll() {
        const length = this.length;
        const chunksNode = this.type.tree_getChunksNode(this.node);
        const chunkCount = Math.ceil(length / this.type.itemsPerChunk);
        const leafNodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.type.chunkDepth, 0, chunkCount);
        const values = new Array(length);
        const itemsPerChunk = this.type.itemsPerChunk; // Prevent many access in for loop below
        const lenFullNodes = Math.floor(length / itemsPerChunk);
        const remainder = length % itemsPerChunk;
        for (let n = 0; n < lenFullNodes; n++) {
            const leafNode = leafNodes[n];
            // TODO: Implement add a fast bulk packed element reader in the elementType
            // ```
            // abstract getValuesFromPackedNode(leafNode: LeafNode, output: V[], indexOffset: number): void;
            // ```
            // if performance here is a problem
            for (let i = 0; i < itemsPerChunk; i++) {
                values[n * itemsPerChunk + i] = this.type.elementType.tree_getFromPackedNode(leafNode, i);
            }
        }
        if (remainder > 0) {
            const leafNode = leafNodes[lenFullNodes];
            for (let i = 0; i < remainder; i++) {
                values[lenFullNodes * itemsPerChunk + i] = this.type.elementType.tree_getFromPackedNode(leafNode, i);
            }
        }
        return values;
    }
}
exports.ArrayBasicTreeView = ArrayBasicTreeView;
//# sourceMappingURL=arrayBasic.js.map