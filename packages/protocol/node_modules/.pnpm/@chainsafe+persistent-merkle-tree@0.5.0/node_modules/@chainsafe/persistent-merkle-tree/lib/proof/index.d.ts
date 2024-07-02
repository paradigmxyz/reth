import { Gindex } from "../gindex";
import { Node } from "../node";
export { computeDescriptor, descriptorToBitlist } from "./compactMulti";
export declare enum ProofType {
    single = "single",
    treeOffset = "treeOffset",
    multi = "multi",
    compactMulti = "compactMulti"
}
/**
 * Serialized proofs are prepended with a single byte, denoting their type
 */
export declare const ProofTypeSerialized: ProofType[];
/**
 * A common merkle proof.
 * A proof for a single leaf in a tree.
 */
export interface SingleProof {
    type: ProofType.single;
    gindex: Gindex;
    leaf: Uint8Array;
    witnesses: Uint8Array[];
}
/**
 * A proof for multiple leaves in a tree.
 *
 * See https://github.com/protolambda/eth-merkle-trees/blob/master/tree_offsets.md
 */
export interface TreeOffsetProof {
    type: ProofType.treeOffset;
    offsets: number[];
    leaves: Uint8Array[];
}
/**
 * A proof for multiple leaves in a tree.
 *
 * See https://github.com/ethereum/consensus-specs/blob/dev/ssz/merkle-proofs.md#merkle-multiproofs
 */
export interface MultiProof {
    type: ProofType.multi;
    leaves: Uint8Array[];
    witnesses: Uint8Array[];
    gindices: Gindex[];
}
export interface CompactMultiProof {
    type: ProofType.compactMulti;
    leaves: Uint8Array[];
    descriptor: Uint8Array;
}
export declare type Proof = SingleProof | TreeOffsetProof | MultiProof | CompactMultiProof;
export interface SingleProofInput {
    type: ProofType.single;
    gindex: Gindex;
}
export interface TreeOffsetProofInput {
    type: ProofType.treeOffset;
    gindices: Gindex[];
}
export interface MultiProofInput {
    type: ProofType.multi;
    gindices: Gindex[];
}
export interface CompactMultiProofInput {
    type: ProofType.compactMulti;
    descriptor: Uint8Array;
}
export declare type ProofInput = SingleProofInput | TreeOffsetProofInput | MultiProofInput | CompactMultiProofInput;
export declare function createProof(rootNode: Node, input: ProofInput): Proof;
export declare function createNodeFromProof(proof: Proof): Node;
export declare function serializeProof(proof: Proof): Uint8Array;
export declare function deserializeProof(data: Uint8Array): Proof;
