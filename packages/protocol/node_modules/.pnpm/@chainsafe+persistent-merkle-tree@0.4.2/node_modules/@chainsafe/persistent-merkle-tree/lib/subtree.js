"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.subtreeFillToContents = exports.subtreeFillToLength = exports.subtreeFillToDepth = void 0;
const node_1 = require("./node");
const zeroNode_1 = require("./zeroNode");
function subtreeFillToDepth(bottom, depth) {
    let node = bottom;
    while (depth > 0) {
        node = new node_1.BranchNode(node, node);
        depth--;
    }
    return node;
}
exports.subtreeFillToDepth = subtreeFillToDepth;
function subtreeFillToLength(bottom, depth, length) {
    const maxLength = 1 << depth;
    if (length > maxLength)
        throw new Error("ERR_TOO_MANY_NODES");
    if (length === maxLength)
        return subtreeFillToDepth(bottom, depth);
    if (depth === 0) {
        if (length === 1)
            return bottom;
        else
            throw new Error("ERR_NAVIGATION");
    }
    if (depth === 1) {
        return new node_1.BranchNode(bottom, length > 1 ? bottom : zeroNode_1.zeroNode(0));
    }
    const pivot = maxLength >> 1;
    if (length <= pivot) {
        return new node_1.BranchNode(subtreeFillToLength(bottom, depth - 1, length), zeroNode_1.zeroNode(depth - 1));
    }
    else {
        return new node_1.BranchNode(subtreeFillToDepth(bottom, depth - 1), subtreeFillToLength(bottom, depth - 1, length - pivot));
    }
}
exports.subtreeFillToLength = subtreeFillToLength;
/**
 * WARNING: Mutates the provided nodes array.
 * TODO: Don't mutate the nodes array.
 */
function subtreeFillToContents(nodes, depth) {
    const maxLength = 2 ** depth;
    if (nodes.length > maxLength) {
        throw new Error(`nodes.length ${nodes.length} over maxIndex at depth ${depth}`);
    }
    if (nodes.length === 0) {
        return zeroNode_1.zeroNode(depth);
    }
    if (depth === 0) {
        return nodes[0];
    }
    if (depth === 1) {
        return nodes.length > 1
            ? // All nodes at depth 1 available
                new node_1.BranchNode(nodes[0], nodes[1])
            : // Pad with zero node
                new node_1.BranchNode(nodes[0], zeroNode_1.zeroNode(0));
    }
    let count = nodes.length;
    for (let d = depth; d > 0; d--) {
        const countRemainder = count % 2;
        const countEven = count - countRemainder;
        // For each depth level compute the new BranchNodes and overwrite the nodes array
        for (let i = 0; i < countEven; i += 2) {
            nodes[i / 2] = new node_1.BranchNode(nodes[i], nodes[i + 1]);
        }
        if (countRemainder > 0) {
            nodes[countEven / 2] = new node_1.BranchNode(nodes[countEven], zeroNode_1.zeroNode(depth - d));
        }
        // If there was remainer, 2 nodes are added to the count
        count = countEven / 2 + countRemainder;
    }
    return nodes[0];
}
exports.subtreeFillToContents = subtreeFillToContents;
