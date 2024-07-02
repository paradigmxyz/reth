import { Node } from "../node";
import { Gindex } from "../gindex";
export declare const ERR_INVALID_NAV = "Invalid tree navigation";
export declare function createSingleProof(rootNode: Node, index: Gindex): [Uint8Array, Uint8Array[]];
export declare function createNodeFromSingleProof(gindex: Gindex, leaf: Uint8Array, witnesses: Uint8Array[]): Node;
