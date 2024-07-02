"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ListBasicTreeView = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const arrayBasic_1 = require("./arrayBasic");
class ListBasicTreeView extends arrayBasic_1.ArrayBasicTreeView {
    constructor(type, tree) {
        super(type, tree);
        this.type = type;
        this.tree = tree;
    }
    /**
     * Adds one value element at the end of the array and adds 1 to the current Tree length.
     */
    push(value) {
        const length = this.length;
        if (length >= this.type.limit) {
            throw Error("Error pushing over limit");
        }
        this.type.tree_setLength(this.tree, length + 1);
        // If in new node..
        if (length % this.type.itemsPerChunk === 0) {
            // TODO: Optimize: This `inNewNode` could be ommitted but it would cause a full navigation in .set()
            // Benchmark the cost of that navigation vs the extra math here
            // TODO: Optimize: prevent double initialization
            const leafNode = persistent_merkle_tree_1.LeafNode.fromZero();
            this.type.elementType.tree_setToPackedNode(leafNode, length, value);
            // Commit immediately
            const chunkIndex = Math.floor(length / this.type.itemsPerChunk);
            this.tree.setNodeAtDepth(this.type.depth, chunkIndex, leafNode);
        }
        else {
            // Re-use .set() since no new node is added
            this.set(length, value);
        }
    }
}
exports.ListBasicTreeView = ListBasicTreeView;
//# sourceMappingURL=listBasic.js.map