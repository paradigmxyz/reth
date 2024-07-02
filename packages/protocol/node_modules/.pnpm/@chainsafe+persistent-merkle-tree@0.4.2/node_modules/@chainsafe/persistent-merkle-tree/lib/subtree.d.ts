import { Node } from "./node";
export declare function subtreeFillToDepth(bottom: Node, depth: number): Node;
export declare function subtreeFillToLength(bottom: Node, depth: number, length: number): Node;
/**
 * WARNING: Mutates the provided nodes array.
 * TODO: Don't mutate the nodes array.
 */
export declare function subtreeFillToContents(nodes: Node[], depth: number): Node;
