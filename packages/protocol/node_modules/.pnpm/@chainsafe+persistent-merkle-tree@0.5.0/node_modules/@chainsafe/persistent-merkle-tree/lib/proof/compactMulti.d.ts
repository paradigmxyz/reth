import { Gindex } from "../gindex";
import { Node } from "../node";
export declare function computeDescriptor(indices: Gindex[]): Uint8Array;
export declare function descriptorToBitlist(descriptor: Uint8Array): boolean[];
export declare function nodeToCompactMultiProof(node: Node, bitlist: boolean[], bitIndex: number): Uint8Array[];
/**
 * Create a Node given a validated bitlist, leaves, and a pointer into the bitlist and leaves
 *
 * Recursive definition
 */
export declare function compactMultiProofToNode(bitlist: boolean[], leaves: Uint8Array[], pointer: {
    bitIndex: number;
    leafIndex: number;
}): Node;
export declare function createCompactMultiProof(rootNode: Node, descriptor: Uint8Array): Uint8Array[];
export declare function createNodeFromCompactMultiProof(leaves: Uint8Array[], descriptor: Uint8Array): Node;
