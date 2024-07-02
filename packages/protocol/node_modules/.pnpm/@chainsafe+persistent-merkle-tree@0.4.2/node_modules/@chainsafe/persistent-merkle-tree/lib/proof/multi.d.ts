import { Gindex } from "../gindex";
import { Node } from "../node";
/**
 * Create an multiproof
 *
 * See https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
 *
 * @param rootNode the root node of the tree
 * @param gindices generalized indices of leaves to include in the proof
 */
export declare function createMultiProof(rootNode: Node, gindices: Gindex[]): [Uint8Array[], Uint8Array[], Gindex[]];
/**
 * Recreate a `Node` given a multiproof
 *
 * See https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
 *
 * @param leaves leaves of a EF multiproof
 * @param witnesses witnesses of a EF multiproof
 * @param gindices generalized indices of the leaves
 */
export declare function createNodeFromMultiProof(leaves: Uint8Array[], witnesses: Uint8Array[], gindices: Gindex[]): Node;
