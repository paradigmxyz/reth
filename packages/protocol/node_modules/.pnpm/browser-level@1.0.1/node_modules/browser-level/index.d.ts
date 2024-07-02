import {
  AbstractLevel,
  AbstractDatabaseOptions,
  NodeCallback,
  AbstractOpenOptions,
  AbstractGetOptions,
  AbstractGetManyOptions,
  AbstractPutOptions,
  AbstractDelOptions,
  AbstractBatchOptions,
  AbstractChainedBatch,
  AbstractIterator,
  AbstractKeyIterator,
  AbstractValueIterator,
  AbstractIteratorOptions,
  AbstractKeyIteratorOptions,
  AbstractValueIteratorOptions,
  AbstractBatchOperation
} from 'abstract-level'

/**
 * An {@link AbstractLevel} database for browsers, backed by [IndexedDB][1].
 *
 * @template KDefault The default type of keys if not overridden on operations.
 * @template VDefault The default type of values if not overridden on operations.
 *
 * [1]: https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API
 */
export class BrowserLevel<KDefault = string, VDefault = string>
  extends AbstractLevel<Uint8Array, KDefault, VDefault> {
  /**
   * Database constructor.
   *
   * @param location The name of the [`IDBDatabase`](https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase)
   * to be opened, as well as the name of the object store within that database. The name
   * of the `IDBDatabase` will be prefixed with {@link DatabaseOptions.prefix}.
   * @param options Options, of which some will be forwarded to {@link open}.
   */
  constructor (location: string, options?: DatabaseOptions<KDefault, VDefault> | undefined)

  /**
   * Location that was passed to the constructor.
   */
  get location (): string

  /**
   * Database name prefix that was passed to the constructor (as `prefix`).
   */
  get namePrefix (): string

  /**
   * Version that was passed to the constructor.
   */
  get version (): number

  /**
   * Delete the IndexedDB database at the given {@link location}.
   */
  static destroy (location: string): Promise<void>
  static destroy (location: string, prefix: string): Promise<void>
  static destroy (location: string, callback: NodeCallback<void>): void
  static destroy (location: string, prefix: string, callback: NodeCallback<void>): void
}

/**
 * Options for the {@link BrowserLevel} constructor.
 */
export interface DatabaseOptions<K, V> extends AbstractDatabaseOptions<K, V> {
  /**
   * Prefix for the `IDBDatabase` name. Can be set to an empty string.
   *
   * @defaultValue `'level-js-'`
   */
  prefix?: string

  /**
   * The version to open the `IDBDatabase` with.
   *
   * @defaultValue `1`
   */
  version?: number | string

  /**
   * An {@link AbstractLevel} option that has no effect on {@link BrowserLevel}.
   */
  createIfMissing?: boolean

  /**
   * An {@link AbstractLevel} option that has no effect on {@link BrowserLevel}.
   */
  errorIfExists?: boolean
}

// Export types so that consumers don't have to guess whether they're extended
export type OpenOptions = AbstractOpenOptions
export type GetOptions<K, V> = AbstractGetOptions<K, V>
export type GetManyOptions<K, V> = AbstractGetManyOptions<K, V>
export type PutOptions<K, V> = AbstractPutOptions<K, V>
export type DelOptions<K> = AbstractDelOptions<K>

export type BatchOptions<K, V> = AbstractBatchOptions<K, V>
export type BatchOperation<TDatabase, K, V> = AbstractBatchOperation<TDatabase, K, V>
export type ChainedBatch<TDatabase, K, V> = AbstractChainedBatch<TDatabase, K, V>

export type Iterator<TDatabase, K, V> = AbstractIterator<TDatabase, K, V>
export type KeyIterator<TDatabase, K> = AbstractKeyIterator<TDatabase, K>
export type ValueIterator<TDatabase, K, V> = AbstractValueIterator<TDatabase, K, V>

export type IteratorOptions<K, V> = AbstractIteratorOptions<K, V>
export type KeyIteratorOptions<K> = AbstractKeyIteratorOptions<K>
export type ValueIteratorOptions<K, V> = AbstractValueIteratorOptions<K, V>
