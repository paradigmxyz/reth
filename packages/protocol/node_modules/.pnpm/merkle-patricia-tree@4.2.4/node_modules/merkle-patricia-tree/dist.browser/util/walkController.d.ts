/// <reference types="node" />
import { BaseTrie } from '..';
import { FoundNodeFunction } from '../baseTrie';
import { PrioritizedTaskExecutor } from '../prioritizedTaskExecutor';
import { BranchNode, Nibbles, TrieNode } from '../trieNode';
/**
 * WalkController is an interface to control how the trie is being traversed.
 */
export declare class WalkController {
    readonly onNode: FoundNodeFunction;
    readonly taskExecutor: PrioritizedTaskExecutor;
    readonly trie: BaseTrie;
    private resolve;
    private reject;
    /**
     * Creates a new WalkController
     * @param onNode - The `FoundNodeFunction` to call if a node is found.
     * @param trie - The `Trie` to walk on.
     * @param poolSize - The size of the task queue.
     */
    private constructor();
    /**
     * Async function to create and start a new walk over a trie.
     * @param onNode - The `FoundNodeFunction to call if a node is found.
     * @param trie - The trie to walk on.
     * @param root - The root key to walk on.
     * @param poolSize - Task execution pool size to prevent OOM errors. Defaults to 500.
     */
    static newWalk(onNode: FoundNodeFunction, trie: BaseTrie, root: Buffer, poolSize?: number): Promise<void>;
    private startWalk;
    /**
     * Run all children of a node. Priority of these nodes are the key length of the children.
     * @param node - Node to get all children of and call onNode on.
     * @param key - The current `key` which would yield the `node` when trying to get this node with a `get` operation.
     */
    allChildren(node: TrieNode, key?: Nibbles): void;
    /**
     * Push a node to the queue. If the queue has places left for tasks, the node is executed immediately, otherwise it is queued.
     * @param nodeRef - Push a node reference to the event queue. This reference is a 32-byte keccak hash of the value corresponding to the `key`.
     * @param key - The current key.
     * @param priority - Optional priority, defaults to key length
     */
    pushNodeToQueue(nodeRef: Buffer, key?: Nibbles, priority?: number): void;
    /**
     * Push a branch of a certain BranchNode to the event queue.
     * @param node - The node to select a branch on. Should be a BranchNode.
     * @param key - The current key which leads to the corresponding node.
     * @param childIndex - The child index to add to the event queue.
     * @param priority - Optional priority of the event, defaults to the total key length.
     */
    onlyBranchIndex(node: BranchNode, key: Nibbles | undefined, childIndex: number, priority?: number): void;
    private processNode;
}
