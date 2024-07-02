"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BranchNodeStruct = void 0;
const persistent_merkle_tree_1 = require("@chainsafe/persistent-merkle-tree");
/**
 * BranchNode whose children's data is represented as a struct, not a tree.
 *
 * This approach is usefull for memory efficiency of data that is not modified often, for example the validators
 * registry in Ethereum consensus `state.validators`. The tradeoff is that getting the hash, are proofs is more
 * expensive because the tree has to be recreated every time.
 */
class BranchNodeStruct extends persistent_merkle_tree_1.Node {
    constructor(valueToNode, value) {
        // First null value is to save an extra variable to check if a node has a root or not
        super(null, 0, 0, 0, 0, 0, 0, 0);
        this.valueToNode = valueToNode;
        this.value = value;
    }
    get rootHashObject() {
        if (this.h0 === null) {
            const node = this.valueToNode(this.value);
            super.applyHash(node.rootHashObject);
        }
        return this;
    }
    get root() {
        return persistent_merkle_tree_1.hashObjectToUint8Array(this.rootHashObject);
    }
    isLeaf() {
        return false;
    }
    get left() {
        return this.valueToNode(this.value).left;
    }
    get right() {
        return this.valueToNode(this.value).right;
    }
}
exports.BranchNodeStruct = BranchNodeStruct;
//# sourceMappingURL=branchNodeStruct.js.map