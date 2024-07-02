"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ListBasicTreeViewDU = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
const arrayBasic_1 = require("./arrayBasic");
class ListBasicTreeViewDU extends arrayBasic_1.ArrayBasicTreeViewDU {
    constructor(type, _rootNode, cache) {
        super(type, _rootNode, cache);
        this.type = type;
        this._rootNode = _rootNode;
    }
    /**
     * Adds one value element at the end of the array and adds 1 to the un-commited ViewDU length
     */
    push(value) {
        if (this._length >= this.type.limit) {
            throw Error("Error pushing over limit");
        }
        // Mutate length before .set()
        this.dirtyLength = true;
        const index = this._length++;
        // If in new node..
        if (index % this.type.itemsPerChunk === 0) {
            // Set a zero node to the nodes array to avoid a navigation downwards in .set()
            const chunkIndex = Math.floor(index / this.type.itemsPerChunk);
            this.nodes[chunkIndex] = persistent_merkle_tree_1.zeroNode(0);
        }
        this.set(index, value);
    }
}
exports.ListBasicTreeViewDU = ListBasicTreeViewDU;
//# sourceMappingURL=listBasic.js.map