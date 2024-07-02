import { Node } from "@chainsafe/persistent-merkle-tree";
declare type JsonPathProp = string | number;
/** Duplicated partial declaration to break circular dependency with CompositeType */
declare type Type = {
    readonly isBasic: boolean;
    /** Tree depth to chunks or LeafNodes */
    readonly depth: number;
    /** Maximum count of LeafNode chunks this type can have when merkleized */
    readonly maxChunkCount: number;
};
/** Duplicated partial declaration to break circular dependency with CompositeType */
declare type CompositeType = Type & {
    /** Return the property's subtype if the property exists */
    getPropertyType(property: JsonPathProp): Type;
    /** Return a leaf node index's property if the index is within bounds */
    getIndexProperty(index: number): JsonPathProp | null;
    /** INTERNAL METHOD: post process `Ǹode` instance created from a proof */
    tree_fromProofNode(node: Node): {
        node: Node;
        done: boolean;
    };
};
/**
 * Navigates and mutates nodes to post process a tree created with `Tree.createFromProof`.
 * Tree returns regular a tree with only BranchNode and LeafNode instances. However, SSZ features
 * non-standard nodes that make proofs for those types to be un-usable. This include:
 * - BranchNodeStruct: Must contain complete data `tree_fromProofNode` transforms a BranchNode and
 *   all of its data into a single BranchNodeStruct instance.
 *
 * @param bitstring Bitstring without the leading "1", since it's only used to compute horizontal indexes.
 */
export declare function treePostProcessFromProofNode(node: Node, type: CompositeType, bitstring?: string, currentDepth?: number): Node;
export {};
//# sourceMappingURL=treePostProcessFromProofNode.d.ts.map