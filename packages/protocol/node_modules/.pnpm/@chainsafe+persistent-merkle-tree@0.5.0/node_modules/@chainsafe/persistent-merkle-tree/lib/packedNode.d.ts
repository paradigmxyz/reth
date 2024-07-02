import { Node } from "./node";
export declare function packedRootsBytesToNode(depth: number, dataView: DataView, start: number, end: number): Node;
/**
 * Optimized deserialization of linear bytes to consecutive leaf nodes
 */
export declare function packedRootsBytesToLeafNodes(dataView: DataView, start: number, end: number): Node[];
/**
 * Optimized serialization of consecutive leave nodes to linear bytes
 */
export declare function packedNodeRootsToBytes(dataView: DataView, start: number, size: number, nodes: Node[]): void;
