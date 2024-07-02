import { HashObject } from "@chainsafe/as-sha256";
/**
 * An immutable binary merkle tree node
 */
export declare abstract class Node implements HashObject {
    /**
     * May be null. This is to save an extra variable to check if a node has a root or not
     */
    h0: number;
    h1: number;
    h2: number;
    h3: number;
    h4: number;
    h5: number;
    h6: number;
    h7: number;
    /** The root hash of the node */
    abstract root: Uint8Array;
    /** The root hash of the node as a `HashObject` */
    abstract rootHashObject: HashObject;
    /** The left child node */
    abstract left: Node;
    /** The right child node */
    abstract right: Node;
    constructor(h0: number, h1: number, h2: number, h3: number, h4: number, h5: number, h6: number, h7: number);
    applyHash(root: HashObject): void;
    /** Returns true if the node is a `LeafNode` */
    abstract isLeaf(): boolean;
}
/**
 * An immutable binary merkle tree node that has a `left` and `right` child
 */
export declare class BranchNode extends Node {
    private _left;
    private _right;
    constructor(_left: Node, _right: Node);
    get rootHashObject(): HashObject;
    get root(): Uint8Array;
    isLeaf(): boolean;
    get left(): Node;
    get right(): Node;
}
/**
 * An immutable binary merkle tree node that has no children
 */
export declare class LeafNode extends Node {
    static fromRoot(root: Uint8Array): LeafNode;
    /**
     * New LeafNode from existing HashObject.
     */
    static fromHashObject(ho: HashObject): LeafNode;
    /**
     * New LeafNode with its internal value set to zero. Consider using `zeroNode(0)` if you don't need to mutate.
     */
    static fromZero(): LeafNode;
    /**
     * LeafNode with HashObject `(uint32, 0, 0, 0, 0, 0, 0, 0)`.
     */
    static fromUint32(uint32: number): LeafNode;
    /**
     * Create a new LeafNode with the same internal values. The returned instance is safe to mutate
     */
    clone(): LeafNode;
    get rootHashObject(): HashObject;
    get root(): Uint8Array;
    isLeaf(): boolean;
    get left(): Node;
    get right(): Node;
    writeToBytes(data: Uint8Array, start: number, size: number): void;
    getUint(uintBytes: number, offsetBytes: number, clipInfinity?: boolean): number;
    getUintBigint(uintBytes: number, offsetBytes: number): bigint;
    setUint(uintBytes: number, offsetBytes: number, value: number, clipInfinity?: boolean): void;
    setUintBigint(uintBytes: number, offsetBytes: number, valueBN: bigint): void;
    bitwiseOrUint(uintBytes: number, offsetBytes: number, value: number): void;
}
export declare type Link = (n: Node) => Node;
export declare function identity(n: Node): Node;
export declare function compose(inner: Link, outer: Link): Link;
export declare function getNodeH(node: Node, hIndex: number): number;
export declare function setNodeH(node: Node, hIndex: number, value: number): void;
export declare function bitwiseOrNodeH(node: Node, hIndex: number, value: number): void;
