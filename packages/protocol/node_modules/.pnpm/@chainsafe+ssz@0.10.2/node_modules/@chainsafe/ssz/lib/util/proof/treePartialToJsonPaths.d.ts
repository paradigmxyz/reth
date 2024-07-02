import { Node } from "@chainsafe/persistent-merkle-tree";
import { CompositeType } from "../../type/composite";
declare type JsonPathProp = string | number;
declare type JsonPath = JsonPathProp[];
export declare enum TreeDataTypeCode {
    witness = "witness",
    partial = "partial",
    complete = "complete"
}
declare type TreeDataTypeWitness = {
    type: TreeDataTypeCode.witness;
};
declare type TreeDataTypePartial = {
    type: TreeDataTypeCode.partial;
    jsonPaths: JsonPath[];
};
declare type TreeDataTypeComplete = {
    type: TreeDataTypeCode.complete;
    jsonPathProps: JsonPathProp[];
};
declare type TreeDataType = TreeDataTypeWitness | TreeDataTypePartial | TreeDataTypeComplete;
export declare function treePartialToJsonPaths(node: Node, type: CompositeType<unknown, unknown, unknown>, bitstring?: string, currentDepth?: number): TreeDataType;
/**
 * Recreate a `Node` given offsets and leaves of a tree-offset proof
 *
 * Recursive definition
 *
 * See https://github.com/protolambda/eth-merkle-trees/blob/master/tree_offsets.md
 */
export declare function treeOffsetProofToNode(offsets: number[], leaves: Uint8Array[]): Node;
export {};
//# sourceMappingURL=treePartialToJsonPaths.d.ts.map