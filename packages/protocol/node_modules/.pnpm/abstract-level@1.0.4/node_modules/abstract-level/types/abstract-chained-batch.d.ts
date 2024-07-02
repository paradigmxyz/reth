import * as Transcoder from 'level-transcoder'
import { AbstractSublevel } from './abstract-sublevel'
import { NodeCallback } from './interfaces'

export class AbstractChainedBatch<TDatabase, KDefault, VDefault> {
  constructor (db: TDatabase)

  /**
   * A reference to the database that created this chained batch.
   */
  db: TDatabase

  /**
   * The number of queued operations on the current batch.
   */
  get length (): number

  /**
   * Queue a _put_ operation on this batch, not committed until {@link write} is
   * called.
   */
  put (key: KDefault, value: VDefault): this

  put<K = KDefault, V = VDefault> (
    key: K,
    value: V,
    options: AbstractChainedBatchPutOptions<TDatabase, K, V>
  ): this

  /**
   * Queue a _del_ operation on this batch, not committed until {@link write} is
   * called.
   */
  del (key: KDefault): this
  del<K = KDefault> (key: K, options: AbstractChainedBatchDelOptions<TDatabase, K>): this

  /**
   * Clear all queued operations on this batch.
   */
  clear (): this

  /**
   * Commit the queued operations for this batch. All operations will be written
   * atomically, that is, they will either all succeed or fail with no partial
   * commits.
   */
  write (): Promise<void>
  write (options: AbstractChainedBatchWriteOptions): Promise<void>
  write (callback: NodeCallback<void>): void
  write (options: AbstractChainedBatchWriteOptions, callback: NodeCallback<void>): void

  /**
   * Free up underlying resources. This should be done even if the chained batch has
   * zero queued operations. Automatically called by {@link write} so normally not
   * necessary to call, unless the intent is to discard a chained batch without
   * committing it.
   */
  close (): Promise<void>
  close (callback: NodeCallback<void>): void
}

/**
 * Options for the {@link AbstractChainedBatch.put} method.
 */
export interface AbstractChainedBatchPutOptions<TDatabase, K, V> {
  /**
   * Custom key encoding for this _put_ operation, used to encode the `key`.
   */
  keyEncoding?: string | Transcoder.PartialEncoder<K> | undefined

  /**
   * Custom value encoding for this _put_ operation, used to encode the `value`.
   */
  valueEncoding?: string | Transcoder.PartialEncoder<V> | undefined

  /**
   * Act as though the _put_ operation is performed on the given sublevel, to similar
   * effect as:
   *
   * ```js
   * await sublevel.batch().put(key, value).write()
   * ```
   *
   * This allows atomically committing data to multiple sublevels. The `key` will be
   * prefixed with the `prefix` of the sublevel, and the `key` and `value` will be
   * encoded by the sublevel (using the default encodings of the sublevel unless
   * {@link keyEncoding} and / or {@link valueEncoding} are provided).
   */
  sublevel?: AbstractSublevel<TDatabase, any, any, any> | undefined
}

/**
 * Options for the {@link AbstractChainedBatch.del} method.
 */
export interface AbstractChainedBatchDelOptions<TDatabase, K> {
  /**
   * Custom key encoding for this _del_ operation, used to encode the `key`.
   */
  keyEncoding?: string | Transcoder.PartialEncoder<K> | undefined

  /**
   * Act as though the _del_ operation is performed on the given sublevel, to similar
   * effect as:
   *
   * ```js
   * await sublevel.batch().del(key).write()
   * ```
   *
   * This allows atomically committing data to multiple sublevels. The `key` will be
   * prefixed with the `prefix` of the sublevel, and the `key` will be encoded by the
   * sublevel (using the default key encoding of the sublevel unless {@link keyEncoding}
   * is provided).
   */
  sublevel?: AbstractSublevel<TDatabase, any, any, any> | undefined
}

/**
 * Options for the {@link AbstractChainedBatch.write} method.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-interface
export interface AbstractChainedBatchWriteOptions {
  // There are no abstract options but implementations may add theirs.
}
