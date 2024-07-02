/// <reference types="node" />
import Semaphore from 'semaphore-async-await';
import { DB, BatchDBOp } from './db';
import { TrieReadStream as ReadStream } from './readStream';
import { WalkController } from './util/walkController';
import { TrieNode, EmbeddedNode, Nibbles } from './trieNode';
import type { LevelUp } from 'levelup';
export declare type Proof = Buffer[];
interface Path {
    node: TrieNode | null;
    remaining: Nibbles;
    stack: TrieNode[];
}
export declare type FoundNodeFunction = (nodeRef: Buffer, node: TrieNode | null, key: Nibbles, walkController: WalkController) => void;
/**
 * The basic trie interface, use with `import { BaseTrie as Trie } from 'merkle-patricia-tree'`.
 * In Ethereum applications stick with the {@link SecureTrie} overlay.
 * The API for the base and the secure interface are about the same.
 */
export declare class Trie {
    /** The root for an empty trie */
    EMPTY_TRIE_ROOT: Buffer;
    protected lock: Semaphore;
    /** The backend DB */
    db: DB;
    private _root;
    private _deleteFromDB;
    /**
     * test
     * @param db - A [levelup](https://github.com/Level/levelup) instance. By default (if the db is `null` or
     * left undefined) creates an in-memory [memdown](https://github.com/Level/memdown) instance.
     * @param root - A `Buffer` for the root of a previously stored trie
     * @param deleteFromDB - Delete nodes from DB on delete operations (disallows switching to an older state root) (default: `false`)
     */
    constructor(db?: LevelUp | null, root?: Buffer, deleteFromDB?: boolean);
    /**
     * Sets the current root of the `trie`
     */
    set root(value: Buffer);
    /**
     * Gets the current root of the `trie`
     */
    get root(): Buffer;
    /**
     * This method is deprecated.
     * Please use {@link Trie.root} instead.
     *
     * @param value
     * @deprecated
     */
    setRoot(value?: Buffer): void;
    /**
     * Checks if a given root exists.
     */
    checkRoot(root: Buffer): Promise<boolean>;
    /**
     * BaseTrie has no checkpointing so return false
     */
    get isCheckpoint(): boolean;
    /**
     * Gets a value given a `key`
     * @param key - the key to search for
     * @param throwIfMissing - if true, throws if any nodes are missing. Used for verifying proofs. (default: false)
     * @returns A Promise that resolves to `Buffer` if a value was found or `null` if no value was found.
     */
    get(key: Buffer, throwIfMissing?: boolean): Promise<Buffer | null>;
    /**
     * Stores a given `value` at the given `key` or do a delete if `value` is empty
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @param key
     * @param value
     * @returns A Promise that resolves once value is stored.
     */
    put(key: Buffer, value: Buffer): Promise<void>;
    /**
     * Deletes a value given a `key` from the trie
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @param key
     * @returns A Promise that resolves once value is deleted.
     */
    del(key: Buffer): Promise<void>;
    /**
     * Tries to find a path to the node for the given key.
     * It returns a `stack` of nodes to the closest node.
     * @param key - the search key
     * @param throwIfMissing - if true, throws if any nodes are missing. Used for verifying proofs. (default: false)
     */
    findPath(key: Buffer, throwIfMissing?: boolean): Promise<Path>;
    /**
     * Walks a trie until finished.
     * @param root
     * @param onFound - callback to call when a node is found. This schedules new tasks. If no tasks are available, the Promise resolves.
     * @returns Resolves when finished walking trie.
     */
    walkTrie(root: Buffer, onFound: FoundNodeFunction): Promise<void>;
    /**
     * @hidden
     * Backwards compatibility
     * @param root -
     * @param onFound -
     */
    _walkTrie(root: Buffer, onFound: FoundNodeFunction): Promise<void>;
    /**
     * Creates the initial node from an empty tree.
     * @private
     */
    _createInitialNode(key: Buffer, value: Buffer): Promise<void>;
    /**
     * Retrieves a node from db by hash.
     */
    lookupNode(node: Buffer | Buffer[]): Promise<TrieNode | null>;
    /**
     * @hidden
     * Backwards compatibility
     * @param node The node hash to lookup from the DB
     */
    _lookupNode(node: Buffer | Buffer[]): Promise<TrieNode | null>;
    /**
     * Updates a node.
     * @private
     * @param key
     * @param value
     * @param keyRemainder
     * @param stack
     */
    _updateNode(k: Buffer, value: Buffer, keyRemainder: Nibbles, stack: TrieNode[]): Promise<void>;
    /**
     * Deletes a node from the trie.
     * @private
     */
    _deleteNode(k: Buffer, stack: TrieNode[]): Promise<void>;
    /**
     * Saves a stack of nodes to the database.
     * @private
     * @param key - the key. Should follow the stack
     * @param stack - a stack of nodes to the value given by the key
     * @param opStack - a stack of levelup operations to commit at the end of this funciton
     */
    _saveStack(key: Nibbles, stack: TrieNode[], opStack: BatchDBOp[]): Promise<void>;
    /**
     * Formats node to be saved by `levelup.batch`.
     * @private
     * @param node - the node to format.
     * @param topLevel - if the node is at the top level.
     * @param opStack - the opStack to push the node's data.
     * @param remove - whether to remove the node (only used for CheckpointTrie).
     * @returns The node's hash used as the key or the rawNode.
     */
    _formatNode(node: TrieNode, topLevel: boolean, opStack: BatchDBOp[], remove?: boolean): Buffer | (EmbeddedNode | null)[];
    /**
     * The given hash of operations (key additions or deletions) are executed on the trie
     * (delete operations are only executed on DB with `deleteFromDB` set to `true`)
     * @example
     * const ops = [
     *    { type: 'del', key: Buffer.from('father') }
     *  , { type: 'put', key: Buffer.from('name'), value: Buffer.from('Yuri Irsenovich Kim') }
     *  , { type: 'put', key: Buffer.from('dob'), value: Buffer.from('16 February 1941') }
     *  , { type: 'put', key: Buffer.from('spouse'), value: Buffer.from('Kim Young-sook') }
     *  , { type: 'put', key: Buffer.from('occupation'), value: Buffer.from('Clown') }
     * ]
     * await trie.batch(ops)
     * @param ops
     */
    batch(ops: BatchDBOp[]): Promise<void>;
    /**
     * Saves the nodes from a proof into the trie. If no trie is provided a new one wil be instantiated.
     * @param proof
     * @param trie
     */
    static fromProof(proof: Proof, trie?: Trie): Promise<Trie>;
    /**
     * prove has been renamed to {@link Trie.createProof}.
     * @deprecated
     * @param trie
     * @param key
     */
    static prove(trie: Trie, key: Buffer): Promise<Proof>;
    /**
     * Creates a proof from a trie and key that can be verified using {@link Trie.verifyProof}.
     * @param trie
     * @param key
     */
    static createProof(trie: Trie, key: Buffer): Promise<Proof>;
    /**
     * Verifies a proof.
     * @param rootHash
     * @param key
     * @param proof
     * @throws If proof is found to be invalid.
     * @returns The value from the key, or null if valid proof of non-existence.
     */
    static verifyProof(rootHash: Buffer, key: Buffer, proof: Proof): Promise<Buffer | null>;
    /**
     * {@link verifyRangeProof}
     */
    static verifyRangeProof(rootHash: Buffer, firstKey: Buffer | null, lastKey: Buffer | null, keys: Buffer[], values: Buffer[], proof: Buffer[] | null): Promise<boolean>;
    /**
     * The `data` event is given an `Object` that has two properties; the `key` and the `value`. Both should be Buffers.
     * @return Returns a [stream](https://nodejs.org/dist/latest-v12.x/docs/api/stream.html#stream_class_stream_readable) of the contents of the `trie`
     */
    createReadStream(): ReadStream;
    /**
     * Creates a new trie backed by the same db.
     */
    copy(): Trie;
    /**
     * Finds all nodes that are stored directly in the db
     * (some nodes are stored raw inside other nodes)
     * called by {@link ScratchReadStream}
     * @private
     */
    _findDbNodes(onFound: FoundNodeFunction): Promise<void>;
    /**
     * Finds all nodes that store k,v values
     * called by {@link TrieReadStream}
     * @private
     */
    _findValueNodes(onFound: FoundNodeFunction): Promise<void>;
}
export {};
