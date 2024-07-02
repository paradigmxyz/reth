"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.deserializeTreeOffsetProof = exports.serializeTreeOffsetProof = exports.computeTreeOffsetProofSerializedLength = exports.createNodeFromTreeOffsetProof = exports.createTreeOffsetProof = exports.treeOffsetProofToNode = exports.nodeToTreeOffsetProof = void 0;
const node_1 = require("../node");
const util_1 = require("./util");
/**
 * Compute offsets and leaves of a tree-offset proof
 *
 * Recursive function
 *
 * See https://github.com/protolambda/eth-merkle-trees/blob/master/tree_offsets.md
 * @param node current node in the tree
 * @param gindex current generalized index in the tree
 * @param proofGindices generalized indices to left include in the proof - must be sorted in-order according to the tree
 */
function nodeToTreeOffsetProof(node, gindex, proofGindices) {
    if (!proofGindices.length || !proofGindices[0].startsWith(gindex)) {
        // there are no proof indices left OR the current subtree contains no remaining proof indices
        return [[], []];
    }
    else if (gindex === proofGindices[0]) {
        // the current node is at the next proof index
        proofGindices.shift();
        return [[], [node.root]];
    }
    else {
        // recursively compute offsets, leaves for the left and right subtree
        const [leftOffsets, leftLeaves] = nodeToTreeOffsetProof(node.left, gindex + "0", proofGindices);
        const [rightOffsets, rightLeaves] = nodeToTreeOffsetProof(node.right, gindex + "1", proofGindices);
        // the offset prepended to the list is # of leaves in the left subtree
        const pivot = leftLeaves.length;
        return [[pivot].concat(leftOffsets, rightOffsets), leftLeaves.concat(rightLeaves)];
    }
}
exports.nodeToTreeOffsetProof = nodeToTreeOffsetProof;
/**
 * Recreate a `Node` given offsets and leaves of a tree-offset proof
 *
 * Recursive definition
 *
 * See https://github.com/protolambda/eth-merkle-trees/blob/master/tree_offsets.md
 */
function treeOffsetProofToNode(offsets, leaves) {
    if (!leaves.length) {
        throw new Error("Proof must contain gt 0 leaves");
    }
    else if (leaves.length === 1) {
        return node_1.LeafNode.fromRoot(leaves[0]);
    }
    else {
        // the offset popped from the list is the # of leaves in the left subtree
        const pivot = offsets[0];
        return new node_1.BranchNode(treeOffsetProofToNode(offsets.slice(1, pivot), leaves.slice(0, pivot)), treeOffsetProofToNode(offsets.slice(pivot), leaves.slice(pivot)));
    }
}
exports.treeOffsetProofToNode = treeOffsetProofToNode;
/**
 * Create a tree-offset proof
 *
 * @param rootNode the root node of the tree
 * @param gindices generalized indices to include in the proof
 */
function createTreeOffsetProof(rootNode, gindices) {
    return nodeToTreeOffsetProof(rootNode, "1", util_1.computeMultiProofBitstrings(gindices.map((g) => g.toString(2))));
}
exports.createTreeOffsetProof = createTreeOffsetProof;
/**
 * Recreate a `Node` given a tree-offset proof
 *
 * @param offsets offsets of a tree-offset proof
 * @param leaves leaves of a tree-offset proof
 */
function createNodeFromTreeOffsetProof(offsets, leaves) {
    // TODO validation
    return treeOffsetProofToNode(offsets, leaves);
}
exports.createNodeFromTreeOffsetProof = createNodeFromTreeOffsetProof;
function computeTreeOffsetProofSerializedLength(offsets, leaves) {
    // add 1 for # of leaves
    return (offsets.length + 1) * 2 + leaves.length * 32;
}
exports.computeTreeOffsetProofSerializedLength = computeTreeOffsetProofSerializedLength;
// Serialized tree offset proof structure:
// # of leaves - 2 bytes
// offsets - 2 bytes each
// leaves - 32 bytes each
function serializeTreeOffsetProof(output, byteOffset, offsets, leaves) {
    const writer = new DataView(output.buffer, output.byteOffset, output.byteLength);
    // set # of leaves
    writer.setUint16(byteOffset, leaves.length, true);
    // set offsets
    const offsetsStartIndex = byteOffset + 2;
    for (let i = 0; i < offsets.length; i++) {
        writer.setUint16(i * 2 + offsetsStartIndex, offsets[i], true);
    }
    // set leaves
    const leavesStartIndex = offsetsStartIndex + offsets.length * 2;
    for (let i = 0; i < leaves.length; i++) {
        output.set(leaves[i], i * 32 + leavesStartIndex);
    }
}
exports.serializeTreeOffsetProof = serializeTreeOffsetProof;
function deserializeTreeOffsetProof(data, byteOffset) {
    const reader = new DataView(data.buffer, data.byteOffset, data.byteLength);
    // get # of leaves
    const leafCount = reader.getUint16(byteOffset, true);
    if (data.length < (leafCount - 1) * 2 + leafCount * 32) {
        throw new Error("Unable to deserialize tree offset proof: not enough bytes");
    }
    // get offsets
    const offsetsStartIndex = byteOffset + 2;
    const offsets = Array.from({ length: leafCount - 1 }, (_, i) => reader.getUint16(i * 2 + offsetsStartIndex, true));
    // get leaves
    const leavesStartIndex = offsetsStartIndex + offsets.length * 2;
    const leaves = Array.from({ length: leafCount }, (_, i) => data.subarray(i * 32 + leavesStartIndex, (i + 1) * 32 + leavesStartIndex));
    return [offsets, leaves];
}
exports.deserializeTreeOffsetProof = deserializeTreeOffsetProof;
