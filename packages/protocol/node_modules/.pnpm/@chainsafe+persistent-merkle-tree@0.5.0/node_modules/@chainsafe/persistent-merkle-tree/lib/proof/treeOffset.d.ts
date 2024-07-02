import { Gindex, GindexBitstring } from "../gindex";
import { Node } from "../node";
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
export declare function nodeToTreeOffsetProof(node: Node, gindex: GindexBitstring, proofGindices: GindexBitstring[]): [number[], Uint8Array[]];
/**
 * Recreate a `Node` given offsets and leaves of a tree-offset proof
 *
 * Recursive definition
 *
 * See https://github.com/protolambda/eth-merkle-trees/blob/master/tree_offsets.md
 */
export declare function treeOffsetProofToNode(offsets: number[], leaves: Uint8Array[]): Node;
/**
 * Create a tree-offset proof
 *
 * @param rootNode the root node of the tree
 * @param gindices generalized indices to include in the proof
 */
export declare function createTreeOffsetProof(rootNode: Node, gindices: Gindex[]): [number[], Uint8Array[]];
/**
 * Recreate a `Node` given a tree-offset proof
 *
 * @param offsets offsets of a tree-offset proof
 * @param leaves leaves of a tree-offset proof
 */
export declare function createNodeFromTreeOffsetProof(offsets: number[], leaves: Uint8Array[]): Node;
export declare function computeTreeOffsetProofSerializedLength(offsets: number[], leaves: Uint8Array[]): number;
export declare function serializeTreeOffsetProof(output: Uint8Array, byteOffset: number, offsets: number[], leaves: Uint8Array[]): void;
export declare function deserializeTreeOffsetProof(data: Uint8Array, byteOffset: number): [number[], Uint8Array[]];
