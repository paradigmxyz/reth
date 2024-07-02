"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.createNodeFromSingleProof = exports.createSingleProof = exports.ERR_INVALID_NAV = void 0;
const node_1 = require("../node");
const gindex_1 = require("../gindex");
exports.ERR_INVALID_NAV = "Invalid tree navigation";
function createSingleProof(rootNode, index) {
    const witnesses = [];
    let node = rootNode;
    for (const i of gindex_1.gindexIterator(index)) {
        if (i) {
            if (node.isLeaf())
                throw new Error(exports.ERR_INVALID_NAV);
            witnesses.push(node.left.root);
            node = node.right;
        }
        else {
            if (node.isLeaf())
                throw new Error(exports.ERR_INVALID_NAV);
            witnesses.push(node.right.root);
            node = node.left;
        }
    }
    return [node.root, witnesses.reverse()];
}
exports.createSingleProof = createSingleProof;
function createNodeFromSingleProof(gindex, leaf, witnesses) {
    let node = node_1.LeafNode.fromRoot(leaf);
    const w = witnesses.slice().reverse();
    while (gindex > 1) {
        const sibling = node_1.LeafNode.fromRoot(w.pop());
        if (gindex % BigInt(2) === BigInt(0)) {
            node = new node_1.BranchNode(node, sibling);
        }
        else {
            node = new node_1.BranchNode(sibling, node);
        }
        gindex = gindex / BigInt(2);
    }
    return node;
}
exports.createNodeFromSingleProof = createNodeFromSingleProof;
