"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.createNodeFromMultiProof = exports.createMultiProof = void 0;
const node_1 = require("../node");
const tree_1 = require("../tree");
const util_1 = require("./util");
/**
 * Create an multiproof
 *
 * See https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
 *
 * @param rootNode the root node of the tree
 * @param gindices generalized indices of leaves to include in the proof
 */
function createMultiProof(rootNode, gindices) {
    const tree = new tree_1.Tree(rootNode);
    const witnessGindices = util_1.computeMultiProofBitstrings(gindices.map((gindex) => gindex.toString(2)), false, util_1.SortOrder.Decreasing);
    const leafGindices = gindices.slice().sort((a, b) => (a < b ? 1 : -1));
    const leaves = leafGindices.map((gindex) => tree.getRoot(gindex));
    const witnesses = witnessGindices.map((gindex) => tree.getRoot(gindex));
    return [leaves, witnesses, leafGindices];
}
exports.createMultiProof = createMultiProof;
/**
 * Recreate a `Node` given a multiproof
 *
 * See https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
 *
 * @param leaves leaves of a EF multiproof
 * @param witnesses witnesses of a EF multiproof
 * @param gindices generalized indices of the leaves
 */
function createNodeFromMultiProof(leaves, witnesses, gindices) {
    if (leaves.length !== gindices.length) {
        throw new Error("Leaves length should equal gindices length");
    }
    const leafBitstrings = gindices.map((gindex) => gindex.toString(2));
    const witnessBitstrings = util_1.computeMultiProofBitstrings(leafBitstrings, false, util_1.SortOrder.Decreasing);
    if (witnessBitstrings.length !== witnesses.length) {
        throw new Error("Witnesses length should equal witnesses gindices length");
    }
    // Algorithm:
    // create an object which tracks key-values for each level
    // pre-load leaves and witnesses into the level object
    // level by level, starting from the bottom,
    // find the sibling, create the parent, store it in the next level up
    // the root is in level 1
    const maxLevel = Math.max(leafBitstrings[0]?.length ?? 0, witnessBitstrings[0]?.length ?? 0);
    const levels = Object.fromEntries(Array.from({ length: maxLevel }, (_, i) => [i + 1, {}]));
    // preload leaves and witnesses
    for (let i = 0; i < leafBitstrings.length; i++) {
        const leafBitstring = leafBitstrings[i];
        const leaf = leaves[i];
        levels[leafBitstring.length][leafBitstring] = node_1.LeafNode.fromRoot(leaf);
    }
    for (let i = 0; i < witnessBitstrings.length; i++) {
        const witnessBitstring = witnessBitstrings[i];
        const witness = witnesses[i];
        levels[witnessBitstring.length][witnessBitstring] = node_1.LeafNode.fromRoot(witness);
    }
    for (let i = maxLevel; i > 1; i--) {
        const level = levels[i];
        const parentLevel = levels[i - 1];
        for (const bitstring of Object.keys(level)) {
            const node = level[bitstring];
            // if the node doesn't exist, we've already processed its sibling
            if (!node) {
                continue;
            }
            const isLeft = bitstring[bitstring.length - 1] === "0";
            const parentBitstring = bitstring.substring(0, bitstring.length - 1);
            const siblingBitstring = parentBitstring + (isLeft ? "1" : "0");
            const siblingNode = level[siblingBitstring];
            if (!siblingNode) {
                throw new Error(`Sibling not found: ${siblingBitstring}`);
            }
            // store the parent node
            const parentNode = isLeft ? new node_1.BranchNode(node, siblingNode) : new node_1.BranchNode(siblingNode, node);
            parentLevel[parentBitstring] = parentNode;
            // delete the used nodes
            delete level[bitstring];
            delete level[siblingBitstring];
        }
    }
    const root = levels[1]["1"];
    if (!root) {
        throw new Error("Internal consistency error: no root found");
    }
    return root;
}
exports.createNodeFromMultiProof = createNodeFromMultiProof;
