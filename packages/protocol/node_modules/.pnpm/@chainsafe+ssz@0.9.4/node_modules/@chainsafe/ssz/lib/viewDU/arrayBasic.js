"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ArrayBasicTreeViewDU = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const abstract_1 = require("./abstract");
class ArrayBasicTreeViewDU extends abstract_1.TreeViewDU {
    constructor(type, _rootNode, cache) {
        super();
        this.type = type;
        this._rootNode = _rootNode;
        this.nodesChanged = new Set();
        this.dirtyLength = false;
        if (cache) {
            this.nodes = cache.nodes;
            this._length = cache.length;
            this.nodesPopulated = cache.nodesPopulated;
        }
        else {
            this.nodes = [];
            this._length = this.type.tree_getLength(_rootNode);
            this.nodesPopulated = false;
        }
    }
    /**
     * Number of elements in the array. Equal to un-commited length of the array
     */
    get length() {
        return this._length;
    }
    get node() {
        return this._rootNode;
    }
    get cache() {
        return {
            nodes: this.nodes,
            length: this._length,
            nodesPopulated: this.nodesPopulated,
        };
    }
    /**
     * Get element at `index`. Returns the Basic element type value directly
     */
    get(index) {
        // First walk through the tree to get the root node for that index
        const chunkIndex = Math.floor(index / this.type.itemsPerChunk);
        let node = this.nodes[chunkIndex];
        if (node === undefined) {
            node = persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, chunkIndex);
            this.nodes[chunkIndex] = node;
        }
        return this.type.elementType.tree_getFromPackedNode(node, index);
    }
    /**
     * Set Basic element type `value` at `index`
     */
    set(index, value) {
        if (index >= this._length) {
            throw Error(`Error setting index over length ${index} > ${this._length}`);
        }
        const chunkIndex = Math.floor(index / this.type.itemsPerChunk);
        // Create new node if current leafNode is not dirty
        let nodeChanged;
        if (this.nodesChanged.has(chunkIndex)) {
            // TODO: This assumes that node has already been populated
            nodeChanged = this.nodes[chunkIndex];
        }
        else {
            const nodePrev = (this.nodes[chunkIndex] ??
                persistent_merkle_tree_1.getNodeAtDepth(this._rootNode, this.type.depth, chunkIndex));
            nodeChanged = nodePrev.clone();
            // Store the changed node in the nodes cache
            this.nodes[chunkIndex] = nodeChanged;
            this.nodesChanged.add(chunkIndex);
        }
        this.type.elementType.tree_setToPackedNode(nodeChanged, index, value);
    }
    /**
     * Get all values of this array as Basic element type values, from index zero to `this.length - 1`
     */
    getAll() {
        if (!this.nodesPopulated) {
            const nodesPrev = this.nodes;
            const chunksNode = this.type.tree_getChunksNode(this.node);
            const chunkCount = Math.ceil(this._length / this.type.itemsPerChunk);
            this.nodes = persistent_merkle_tree_1.getNodesAtDepth(chunksNode, this.type.chunkDepth, 0, chunkCount);
            // Re-apply changed nodes
            for (const index of this.nodesChanged) {
                this.nodes[index] = nodesPrev[index];
            }
            this.nodesPopulated = true;
        }
        const values = new Array(this._length);
        const itemsPerChunk = this.type.itemsPerChunk; // Prevent many access in for loop below
        const lenFullNodes = Math.floor(this._length / itemsPerChunk);
        const remainder = this._length % itemsPerChunk;
        // TODO Optimize: caching the variables used in the loop above it
        for (let n = 0; n < lenFullNodes; n++) {
            const leafNode = this.nodes[n];
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
            const leafNode = this.nodes[lenFullNodes];
            for (let i = 0; i < remainder; i++) {
                values[lenFullNodes * itemsPerChunk + i] = this.type.elementType.tree_getFromPackedNode(leafNode, i);
            }
        }
        return values;
    }
    commit() {
        if (this.nodesChanged.size === 0) {
            return;
        }
        // Numerical sort ascending
        const indexes = Array.from(this.nodesChanged.keys()).sort((a, b) => a - b);
        const nodes = new Array(indexes.length);
        for (let i = 0; i < indexes.length; i++) {
            nodes[i] = this.nodes[indexes[i]];
        }
        const chunksNode = this.type.tree_getChunksNode(this._rootNode);
        // TODO: Ensure fast setNodesAtDepth() method is correct
        const newChunksNode = persistent_merkle_tree_1.setNodesAtDepth(chunksNode, this.type.chunkDepth, indexes, nodes);
        this._rootNode = this.type.tree_setChunksNode(this._rootNode, newChunksNode, this.dirtyLength ? this._length : undefined);
        this.nodesChanged.clear();
        this.dirtyLength = false;
    }
    clearCache() {
        this.nodes = [];
        this.nodesPopulated = false;
        // Must clear nodesChanged, otherwise a subsequent commit call will break, because it assumes a node is there
        this.nodesChanged.clear();
        // Reset cached length only if it has been mutated
        if (this.dirtyLength) {
            this._length = this.type.tree_getLength(this._rootNode);
            this.dirtyLength = false;
        }
    }
}
exports.ArrayBasicTreeViewDU = ArrayBasicTreeViewDU;
//# sourceMappingURL=arrayBasic.js.map