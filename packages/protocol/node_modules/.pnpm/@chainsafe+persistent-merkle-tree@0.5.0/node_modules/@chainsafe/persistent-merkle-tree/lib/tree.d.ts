import { Gindex, GindexBitstring } from "./gindex";
import { Node } from "./node";
import { Proof, ProofInput } from "./proof";
export declare type Hook = (newRootNode: Node) => void;
/**
 * Binary merkle tree
 *
 * Wrapper around immutable `Node` to support mutability.
 *
 * Mutability between a parent tree and subtree is achieved by maintaining a `hook` callback, which updates the parent when the subtree is updated.
 */
export declare class Tree {
    private _rootNode;
    private hook?;
    constructor(node: Node, hook?: Hook);
    /**
     * Create a `Tree` from a `Proof` object
     */
    static createFromProof(proof: Proof): Tree;
    /**
     * The root node of the tree
     */
    get rootNode(): Node;
    /**
     *
     * Setting the root node will trigger a call to the tree's `hook` if it exists.
     */
    set rootNode(newRootNode: Node);
    /**
     * The root hash of the tree
     */
    get root(): Uint8Array;
    /**
     * Return a copy of the tree
     */
    clone(): Tree;
    /**
     * Return the subtree at the specified gindex.
     *
     * Note: The returned subtree will have a `hook` attached to the parent tree.
     * Updates to the subtree will result in updates to the parent.
     */
    getSubtree(index: Gindex | GindexBitstring): Tree;
    /**
     * Return the node at the specified gindex.
     */
    getNode(gindex: Gindex | GindexBitstring): Node;
    /**
     * Return the node at the specified depth and index.
     *
     * Supports index up to `Number.MAX_SAFE_INTEGER`.
     */
    getNodeAtDepth(depth: number, index: number): Node;
    /**
     * Return the hash at the specified gindex.
     */
    getRoot(index: Gindex | GindexBitstring): Uint8Array;
    /**
     * Set the node at at the specified gindex.
     */
    setNode(gindex: Gindex | GindexBitstring, n: Node): void;
    /**
     * Traverse to the node at the specified gindex,
     * then apply the function to get a new node and set the node at the specified gindex with the result.
     *
     * This is a convenient method to avoid traversing the tree 2 times to
     * get and set.
     */
    setNodeWithFn(gindex: Gindex | GindexBitstring, getNewNode: (node: Node) => Node): void;
    /**
     * Set the node at the specified depth and index.
     *
     * Supports index up to `Number.MAX_SAFE_INTEGER`.
     */
    setNodeAtDepth(depth: number, index: number, node: Node): void;
    /**
     * Set the hash at the specified gindex.
     *
     * Note: This will set a new `LeafNode` at the specified gindex.
     */
    setRoot(index: Gindex | GindexBitstring, root: Uint8Array): void;
    /**
     * Fast read-only iteration
     * In-order traversal of nodes at `depth`
     * starting from the `startIndex`-indexed node
     * iterating through `count` nodes
     *
     * Supports index up to `Number.MAX_SAFE_INTEGER`.
     */
    getNodesAtDepth(depth: number, startIndex: number, count: number): Node[];
    /**
     * Fast read-only iteration
     * In-order traversal of nodes at `depth`
     * starting from the `startIndex`-indexed node
     * iterating through `count` nodes
     *
     * Supports index up to `Number.MAX_SAFE_INTEGER`.
     */
    iterateNodesAtDepth(depth: number, startIndex: number, count: number): IterableIterator<Node>;
    /**
     * Return a merkle proof for the node at the specified gindex.
     */
    getSingleProof(index: Gindex): Uint8Array[];
    /**
     * Return a merkle proof for the proof input.
     *
     * This method can be used to create multiproofs.
     */
    getProof(input: ProofInput): Proof;
}
/**
 * Return the node at the specified gindex.
 */
export declare function getNode(rootNode: Node, gindex: Gindex | GindexBitstring): Node;
/**
 * Set the node at at the specified gindex.
 * Returns the new root node.
 */
export declare function setNode(rootNode: Node, gindex: Gindex | GindexBitstring, n: Node): Node;
/**
 * Traverse to the node at the specified gindex,
 * then apply the function to get a new node and set the node at the specified gindex with the result.
 *
 * This is a convenient method to avoid traversing the tree 2 times to
 * get and set.
 *
 * Returns the new root node.
 */
export declare function setNodeWithFn(rootNode: Node, gindex: Gindex | GindexBitstring, getNewNode: (node: Node) => Node): Node;
/**
 * Supports index up to `Number.MAX_SAFE_INTEGER`.
 */
export declare function getNodeAtDepth(rootNode: Node, depth: number, index: number): Node;
/**
 * Supports index up to `Number.MAX_SAFE_INTEGER`.
 */
export declare function setNodeAtDepth(rootNode: Node, nodesDepth: number, index: number, nodeChanged: Node): Node;
/**
 * Set multiple nodes in batch, editing and traversing nodes strictly once.
 *
 * - gindexes MUST be sorted in ascending order beforehand.
 * - All gindexes must be at the exact same depth.
 * - Depth must be > 0, if 0 just replace the root node.
 *
 * Strategy: for each gindex in `gindexes` navigate to the depth of its parent,
 * and create a new parent. Then calculate the closest common depth with the next
 * gindex and navigate upwards creating or caching nodes as necessary. Loop and repeat.
 *
 * Supports index up to `Number.MAX_SAFE_INTEGER`.
 */
export declare function setNodesAtDepth(rootNode: Node, nodesDepth: number, indexes: number[], nodes: Node[]): Node;
/**
 * Fast read-only iteration
 * In-order traversal of nodes at `depth`
 * starting from the `startIndex`-indexed node
 * iterating through `count` nodes
 *
 * **Strategy**
 * 1. Navigate down to parentDepth storing a stack of parents
 * 2. At target level push current node
 * 3. Go up to the first level that navigated left
 * 4. Repeat (1) for next index
 */
export declare function getNodesAtDepth(rootNode: Node, depth: number, startIndex: number, count: number): Node[];
/**
 * @see getNodesAtDepth but instead of pushing to an array, it yields
 */
export declare function iterateNodesAtDepth(rootNode: Node, depth: number, startIndex: number, count: number): IterableIterator<Node>;
/**
 * Zero's all nodes right of index with constant depth of `nodesDepth`.
 *
 * For example, zero-ing this tree at depth 2 after index 0
 * ```
 *    X              X
 *  X   X    ->    X   0
 * X X X X        X 0 0 0
 * ```
 *
 * Or, zero-ing this tree at depth 3 after index 2
 * ```
 *        X                     X
 *    X       X             X       0
 *  X   X   X   X    ->   X   X   0   0
 * X X X X X X X X       X X X 0 0 0 0 0
 * ```
 *
 * The strategy is to first navigate down to `nodesDepth` and `index` and keep a stack of parents.
 * Then navigate up re-binding:
 * - If navigated to the left rebind with zeroNode()
 * - If navigated to the right rebind with parent.left from the stack
 */
export declare function treeZeroAfterIndex(rootNode: Node, nodesDepth: number, index: number): Node;
