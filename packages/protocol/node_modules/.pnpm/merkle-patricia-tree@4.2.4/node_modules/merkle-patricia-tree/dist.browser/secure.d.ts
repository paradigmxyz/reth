/// <reference types="node" />
import { CheckpointTrie } from './checkpointTrie';
import { Proof } from './baseTrie';
/**
 * You can create a secure Trie where the keys are automatically hashed
 * using **keccak256** by using `import { SecureTrie as Trie } from 'merkle-patricia-tree'`.
 * It has the same methods and constructor as `Trie`.
 * @class SecureTrie
 * @extends Trie
 * @public
 */
export declare class SecureTrie extends CheckpointTrie {
    constructor(...args: any);
    /**
     * Gets a value given a `key`
     * @param key - the key to search for
     * @returns A Promise that resolves to `Buffer` if a value was found or `null` if no value was found.
     */
    get(key: Buffer): Promise<Buffer | null>;
    /**
     * Stores a given `value` at the given `key`.
     * For a falsey value, use the original key to avoid double hashing the key.
     * @param key
     * @param value
     */
    put(key: Buffer, val: Buffer): Promise<void>;
    /**
     * Deletes a value given a `key`.
     * @param key
     */
    del(key: Buffer): Promise<void>;
    /**
     * prove has been renamed to {@link SecureTrie.createProof}.
     * @deprecated
     * @param trie
     * @param key
     */
    static prove(trie: SecureTrie, key: Buffer): Promise<Proof>;
    /**
     * Creates a proof that can be verified using {@link SecureTrie.verifyProof}.
     * @param trie
     * @param key
     */
    static createProof(trie: SecureTrie, key: Buffer): Promise<Proof>;
    /**
     * Verifies a proof.
     * @param rootHash
     * @param key
     * @param proof
     * @throws If proof is found to be invalid.
     * @returns The value from the key.
     */
    static verifyProof(rootHash: Buffer, key: Buffer, proof: Proof): Promise<Buffer | null>;
    /**
     * Verifies a range proof.
     */
    static verifyRangeProof(rootHash: Buffer, firstKey: Buffer | null, lastKey: Buffer | null, keys: Buffer[], values: Buffer[], proof: Buffer[] | null): Promise<boolean>;
    /**
     * Returns a copy of the underlying trie with the interface of SecureTrie.
     * @param includeCheckpoints - If true and during a checkpoint, the copy will contain the checkpointing metadata and will use the same scratch as underlying db.
     */
    copy(includeCheckpoints?: boolean): SecureTrie;
}
