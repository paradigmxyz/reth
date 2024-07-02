import type { BranchNode, ExtensionNode, LeafNode } from './trie'
import type { WalkController } from './util/walkController'

export type TrieNode = BranchNode | ExtensionNode | LeafNode

export type Nibbles = number[]

// Branch and extension nodes might store
// hash to next node, or embed it if its len < 32
export type EmbeddedNode = Buffer | Buffer[]

export type Proof = Buffer[]

export type FoundNodeFunction = (
  nodeRef: Buffer,
  node: TrieNode | null,
  key: Nibbles,
  walkController: WalkController
) => void

export type HashKeysFunction = (msg: Uint8Array) => Uint8Array

export interface TrieOpts {
  /**
   * A database instance.
   */
  db?: DB

  /**
   * A `Buffer` for the root of a previously stored trie
   */
  root?: Buffer

  /**
   * Create as a secure Trie where the keys are automatically hashed using the
   * **keccak256** hash function or alternatively the custom hash function provided.
   * Default: `false`
   *
   * This is the flavor of the Trie which is used in production Ethereum networks
   * like Ethereum Mainnet.
   *
   * Note: This functionality has been refactored along the v5 release and was before
   * provided as a separate inherited class `SecureTrie`. Just replace with `Trie`
   * instantiation with `useKeyHashing` set to `true`.
   */
  useKeyHashing?: boolean

  /**
   * Hash function used for hashing trie node and securing key.
   */
  useKeyHashingFunction?: HashKeysFunction

  /**
   * Store the root inside the database after every `write` operation
   */
  useRootPersistence?: boolean

  /**
   * Flag to prune the trie. When set to `true`, each time a value is overridden,
   * unreachable nodes will be pruned (deleted) from the trie
   */
  useNodePruning?: boolean
}

export type TrieOptsWithDefaults = TrieOpts & {
  useKeyHashing: boolean
  useKeyHashingFunction: HashKeysFunction
  useRootPersistence: boolean
  useNodePruning: boolean
}

export type BatchDBOp = PutBatch | DelBatch

export interface PutBatch {
  type: 'put'
  key: Buffer
  value: Buffer
}

export interface DelBatch {
  type: 'del'
  key: Buffer
}

export interface DB {
  /**
   * Retrieves a raw value from leveldb.
   * @param key
   * @returns A Promise that resolves to `Buffer` if a value is found or `null` if no value is found.
   */
  get(key: Buffer): Promise<Buffer | null>

  /**
   * Writes a value directly to leveldb.
   * @param key The key as a `Buffer`
   * @param value The value to be stored
   */
  put(key: Buffer, val: Buffer): Promise<void>

  /**
   * Removes a raw value in the underlying leveldb.
   * @param keys
   */
  del(key: Buffer): Promise<void>

  /**
   * Performs a batch operation on db.
   * @param opStack A stack of levelup operations
   */
  batch(opStack: BatchDBOp[]): Promise<void>

  /**
   * Returns a copy of the DB instance, with a reference
   * to the **same** underlying leveldb instance.
   */
  copy(): DB
}

export type Checkpoint = {
  // We cannot use a Buffer => Buffer map directly. If you create two Buffers with the same internal value,
  // then when setting a value on the Map, it actually creates two indices.
  keyValueMap: Map<string, Buffer | null>
  root: Buffer
}

export const ROOT_DB_KEY = Buffer.from('__root__')
