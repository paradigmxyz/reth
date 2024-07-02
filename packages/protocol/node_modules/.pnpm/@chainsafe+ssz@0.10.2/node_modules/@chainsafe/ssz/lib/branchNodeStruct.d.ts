import { HashObject } from "@chainsafe/as-sha256";
import { Node } from "@chainsafe/persistent-merkle-tree";
/**
 * BranchNode whose children's data is represented as a struct, not a tree.
 *
 * This approach is usefull for memory efficiency of data that is not modified often, for example the validators
 * registry in Ethereum consensus `state.validators`. The tradeoff is that getting the hash, are proofs is more
 * expensive because the tree has to be recreated every time.
 */
export declare class BranchNodeStruct<T> extends Node {
    private readonly valueToNode;
    readonly value: T;
    constructor(valueToNode: (value: T) => Node, value: T);
    get rootHashObject(): HashObject;
    get root(): Uint8Array;
    isLeaf(): boolean;
    get left(): Node;
    get right(): Node;
}
//# sourceMappingURL=branchNodeStruct.d.ts.map