"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.computeMultiProofBitstrings = exports.SortOrder = exports.filterParentBitstrings = exports.sortDecreasingBitstrings = exports.sortInOrderBitstrings = exports.computeProofBitstrings = exports.computeProofGindices = void 0;
const gindex_1 = require("../gindex");
// Not currently in use, but simpler implementation useful for testing
/**
 * Compute both the path and branch indices
 *
 * Path indices are parent indices upwards toward the root
 * Branch indices are witnesses required for a merkle proof
 */
function computeProofGindices(gindex) {
    const path = new Set();
    const branch = new Set();
    let g = gindex;
    while (g > 1) {
        path.add(g);
        branch.add(gindex_1.gindexSibling(g));
        g = gindex_1.gindexParent(g);
    }
    return { path, branch };
}
exports.computeProofGindices = computeProofGindices;
/**
 * Compute both the path and branch indices
 *
 * Path indices are parent indices upwards toward the root
 * Branch indices are witnesses required for a merkle proof
 */
function computeProofBitstrings(gindex) {
    const path = new Set();
    const branch = new Set();
    let g = gindex;
    while (g.length > 1) {
        path.add(g);
        const lastBit = g[g.length - 1];
        const parent = g.substring(0, g.length - 1);
        branch.add(parent + (Number(lastBit) ^ 1));
        g = parent;
    }
    return { path, branch };
}
exports.computeProofBitstrings = computeProofBitstrings;
/**
 * Sort generalized indices in-order
 * @param bitLength maximum bit length of generalized indices to sort
 */
function sortInOrderBitstrings(gindices, bitLength) {
    if (!gindices.length) {
        return [];
    }
    return gindices
        .map((g) => g.padEnd(bitLength))
        .sort()
        .map((g) => g.trim());
}
exports.sortInOrderBitstrings = sortInOrderBitstrings;
/**
 * Sort generalized indices in decreasing order
 */
function sortDecreasingBitstrings(gindices) {
    if (!gindices.length) {
        return [];
    }
    return gindices.sort((a, b) => {
        if (a.length < b.length) {
            return 1;
        }
        else if (b.length < a.length) {
            return -1;
        }
        let aPos0 = a.indexOf("0");
        let bPos0 = b.indexOf("0");
        // eslint-disable-next-line no-constant-condition
        while (true) {
            if (aPos0 === -1) {
                return -1;
            }
            else if (bPos0 === -1) {
                return 1;
            }
            if (aPos0 < bPos0) {
                return 1;
            }
            else if (bPos0 < aPos0) {
                return -1;
            }
            aPos0 = a.indexOf("0", aPos0 + 1);
            bPos0 = b.indexOf("0", bPos0 + 1);
        }
    });
}
exports.sortDecreasingBitstrings = sortDecreasingBitstrings;
/**
 * Filter out parent generalized indices
 */
function filterParentBitstrings(gindices) {
    const sortedBitstrings = gindices.slice().sort((a, b) => a.length - b.length);
    const filtered = [];
    outer: for (let i = 0; i < sortedBitstrings.length; i++) {
        const bsA = sortedBitstrings[i];
        for (let j = i + 1; j < sortedBitstrings.length; j++) {
            const bsB = sortedBitstrings[j];
            if (bsB.startsWith(bsA)) {
                continue outer;
            }
        }
        filtered.push(bsA);
    }
    return filtered;
}
exports.filterParentBitstrings = filterParentBitstrings;
var SortOrder;
(function (SortOrder) {
    SortOrder[SortOrder["InOrder"] = 0] = "InOrder";
    SortOrder[SortOrder["Decreasing"] = 1] = "Decreasing";
    SortOrder[SortOrder["Unsorted"] = 2] = "Unsorted";
})(SortOrder = exports.SortOrder || (exports.SortOrder = {}));
/**
 * Return the set of generalized indices required for a multiproof
 * This may include all leaves and any necessary witnesses
 * @param gindices leaves to include in proof
 * @returns all generalized indices required for a multiproof (leaves and witnesses), deduplicated and sorted
 */
function computeMultiProofBitstrings(gindices, includeLeaves = true, sortOrder = SortOrder.InOrder) {
    const leaves = filterParentBitstrings(gindices);
    // Maybe initialize the proof indices with the leaves
    const proof = new Set(includeLeaves ? leaves : []);
    const paths = new Set();
    const branches = new Set();
    // Collect all path indices and all branch indices
    let maxBitLength = 1;
    for (const gindex of leaves) {
        if (gindex.length > maxBitLength)
            maxBitLength = gindex.length;
        const { path, branch } = computeProofBitstrings(gindex);
        path.forEach((g) => paths.add(g));
        branch.forEach((g) => branches.add(g));
    }
    // Remove all branches that are included in the paths
    paths.forEach((g) => branches.delete(g));
    // Add all remaining branches to the leaves
    branches.forEach((g) => proof.add(g));
    switch (sortOrder) {
        case SortOrder.InOrder:
            return sortInOrderBitstrings(Array.from(proof), maxBitLength);
        case SortOrder.Decreasing:
            return sortDecreasingBitstrings(Array.from(proof));
        case SortOrder.Unsorted:
            return Array.from(proof);
    }
}
exports.computeMultiProofBitstrings = computeMultiProofBitstrings;
