import {
  AbstractLevel,
  AbstractDatabaseOptions,
  AbstractOpenOptions,
  AbstractGetOptions,
  AbstractGetManyOptions,
  AbstractPutOptions,
  AbstractDelOptions,
  AbstractBatchOperation,
  AbstractBatchOptions,
  AbstractChainedBatch,
  AbstractChainedBatchWriteOptions,
  AbstractIteratorOptions,
  AbstractIterator,
  AbstractKeyIterator,
  AbstractKeyIteratorOptions,
  AbstractValueIterator,
  AbstractValueIteratorOptions,
  Transcoder,
  NodeCallback
} from 'abstract-level'

/**
 * An {@link AbstractLevel} database backed by
 * [LevelDB](https://github.com/google/leveldb).
 *
 * @template KDefault The default type of keys if not overridden on operations.
 * @template VDefault The default type of values if not overridden on operations.
 */
declare class ClassicLevel<KDefault = string, VDefault = string>
  extends AbstractLevel<string | Buffer | Uint8Array, KDefault, VDefault> {
  /**
   * Database constructor.
   *
   * @param location Directory path (relative or absolute) where LevelDB will
   * store its files.
   * @param options Options, of which some will be forwarded to {@link open}.
   */
  constructor (
    location: string,
    options?: DatabaseOptions<KDefault, VDefault> | undefined
  )

  /**
   * Location that was passed to the constructor.
   */
  get location (): string

  open (): Promise<void>
  open (options: OpenOptions): Promise<void>
  open (callback: NodeCallback<void>): void
  open (options: OpenOptions, callback: NodeCallback<void>): void

  get (key: KDefault): Promise<VDefault>
  get (key: KDefault, callback: NodeCallback<VDefault>): void
  get<K = KDefault, V = VDefault> (key: K, options: GetOptions<K, V>): Promise<V>
  get<K = KDefault, V = VDefault> (key: K, options: GetOptions<K, V>, callback: NodeCallback<V>): void

  getMany (keys: KDefault[]): Promise<VDefault[]>
  getMany (keys: KDefault[], callback: NodeCallback<VDefault[]>): void
  getMany<K = KDefault, V = VDefault> (keys: K[], options: GetManyOptions<K, V>): Promise<V[]>
  getMany<K = KDefault, V = VDefault> (keys: K[], options: GetManyOptions<K, V>, callback: NodeCallback<V[]>): void

  put (key: KDefault, value: VDefault): Promise<void>
  put (key: KDefault, value: VDefault, callback: NodeCallback<void>): void
  put<K = KDefault, V = VDefault> (key: K, value: V, options: PutOptions<K, V>): Promise<void>
  put<K = KDefault, V = VDefault> (key: K, value: V, options: PutOptions<K, V>, callback: NodeCallback<void>): void

  del (key: KDefault): Promise<void>
  del (key: KDefault, callback: NodeCallback<void>): void
  del<K = KDefault> (key: K, options: DelOptions<K>): Promise<void>
  del<K = KDefault> (key: K, options: DelOptions<K>, callback: NodeCallback<void>): void

  batch (operations: Array<BatchOperation<typeof this, KDefault, VDefault>>): Promise<void>
  batch (operations: Array<BatchOperation<typeof this, KDefault, VDefault>>, callback: NodeCallback<void>): void
  batch<K = KDefault, V = VDefault> (operations: Array<BatchOperation<typeof this, K, V>>, options: BatchOptions<K, V>): Promise<void>
  batch<K = KDefault, V = VDefault> (operations: Array<BatchOperation<typeof this, K, V>>, options: BatchOptions<K, V>, callback: NodeCallback<void>): void
  batch (): ChainedBatch<typeof this, KDefault, VDefault>

  iterator (): Iterator<typeof this, KDefault, VDefault>
  iterator<K = KDefault, V = VDefault> (options: IteratorOptions<K, V>): Iterator<typeof this, K, V>

  keys (): KeyIterator<typeof this, KDefault>
  keys<K = KDefault> (options: KeyIteratorOptions<K>): KeyIterator<typeof this, K>

  values (): ValueIterator<typeof this, KDefault, VDefault>
  values<K = KDefault, V = VDefault> (options: ValueIteratorOptions<K, V>): ValueIterator<typeof this, K, V>

  /**
   * Get the approximate number of bytes of file system space used by the range
   * `[start..end)`.
   */
  approximateSize (start: KDefault, end: KDefault): Promise<number>
  approximateSize (start: KDefault, end: KDefault, callback: NodeCallback<number>): void
  approximateSize<K = KDefault> (start: K, end: K, options: StartEndOptions<K>): Promise<number>
  approximateSize<K = KDefault> (start: K, end: K, options: StartEndOptions<K>, callback: NodeCallback<number>): void

  /**
   * Manually trigger a database compaction in the range `[start..end)`.
   */
  compactRange (start: KDefault, end: KDefault): Promise<void>
  compactRange (start: KDefault, end: KDefault, callback: NodeCallback<void>): void
  compactRange<K = KDefault> (start: K, end: K, options: StartEndOptions<K>): Promise<void>
  compactRange<K = KDefault> (start: K, end: K, options: StartEndOptions<K>, callback: NodeCallback<void>): void

  /**
   * Get internal details from LevelDB.
   */
  getProperty (property: string): string

  /**
   * Completely remove an existing LevelDB database directory. Can be used in
   * place of a full directory removal to only remove LevelDB-related files.
   */
  static destroy (location: string): Promise<void>
  static destroy (location: string, callback: NodeCallback<void>): void

  /**
   * Attempt a restoration of a damaged database. Can also be used to perform
   * a compaction of the LevelDB log into table files.
   */
  static repair (location: string): Promise<void>
  static repair (location: string, callback: NodeCallback<void>): void
}

/**
 * Options for the database constructor.
 */
export interface DatabaseOptions<K, V>
  extends AbstractDatabaseOptions<K, V>, Omit<OpenOptions, 'passive'> {}

/**
 * Options for the {@link ClassicLevel.open} method.
 */
export interface OpenOptions extends AbstractOpenOptions {
  /**
   * Unless set to `false`, all _compressible_ data will be run through the
   * Snappy compression algorithm before being stored. Snappy is very fast so
   * leave this on unless you have good reason to turn it off.
   *
   * @defaultValue `true`
   */
  compression?: boolean | undefined

  /**
   * The size (in bytes) of the in-memory
   * [LRU](http://en.wikipedia.org/wiki/Least_Recently_Used)
   * cache with frequently used uncompressed block contents.
   *
   * @defaultValue `8 * 1024 * 1024`
   */
  cacheSize?: number | undefined

  /**
   * The maximum size (in bytes) of the log (in memory and stored in the `.log`
   * file on disk). Beyond this size, LevelDB will convert the log data to the
   * first level of sorted table files. From LevelDB documentation:
   *
   * > Larger values increase performance, especially during bulk loads. Up to
   * > two write buffers may be held in memory at the same time, so you may
   * > wish to adjust this parameter to control memory usage. Also, a larger
   * > write buffer will result in a longer recovery time the next time the
   * > database is opened.
   *
   * @defaultValue `4 * 1024 * 1024`
   */
  writeBufferSize?: number | undefined

  /**
   * The _approximate_ size of the blocks that make up the table files. The
   * size relates to uncompressed data (hence "approximate"). Blocks are
   * indexed in the table file and entry-lookups involve reading an entire
   * block and parsing to discover the required entry.
   *
   * @defaultValue `4096`
   */
  blockSize?: number | undefined

  /**
   * The maximum number of files that LevelDB is allowed to have open at a
   * time. If your database is likely to have a large working set, you may
   * increase this value to prevent file descriptor churn. To calculate the
   * number of files required for your working set, divide your total data size
   * by `maxFileSize`.
   *
   * @defaultValue 1000
   */
  maxOpenFiles?: number | undefined

  /**
   * The number of entries before restarting the "delta encoding" of keys
   * within blocks. Each "restart" point stores the full key for the entry,
   * between restarts, the common prefix of the keys for those entries is
   * omitted. Restarts are similar to the concept of keyframes in video
   * encoding and are used to minimise the amount of space required to store
   * keys. This is particularly helpful when using deep namespacing / prefixing
   * in your keys.
   *
   * @defaultValue `16`
   */
  blockRestartInterval?: number | undefined

  /**
   * The maximum amount of bytes to write to a file before switching to a new
   * one. From LevelDB documentation:
   *
   * > If your filesystem is more efficient with larger files, you could
   * > consider increasing the value. The downside will be longer compactions
   * > and hence longer latency / performance hiccups. Another reason to
   * > increase this parameter might be when you are initially populating a
   * > large database.
   *
   * @defaultValue `2 * 1024 * 1024`
   */
  maxFileSize?: number | undefined

  /**
   * Allows multi-threaded access to a single DB instance for sharing a DB
   * across multiple worker threads within the same process.
   * 
   * @defaultValue `false`
   */
  multithreading?: boolean | undefined
}

/**
 * Additional options for the {@link ClassicLevel.get} and {@link ClassicLevel.getMany}
 * methods.
 */
declare interface ReadOptions {
  /**
   * Unless set to `false`, LevelDB will fill its in-memory
   * [LRU](http://en.wikipedia.org/wiki/Least_Recently_Used) cache with data
   * that was read.
   *
   * @defaultValue `true`
   */
  fillCache?: boolean | undefined
}

/**
 * Options for the {@link ClassicLevel.get} method.
 */
export interface GetOptions<K, V> extends AbstractGetOptions<K, V>, ReadOptions {}

/**
 * Options for the {@link ClassicLevel.getMany} method.
 */
export interface GetManyOptions<K, V> extends AbstractGetManyOptions<K, V>, ReadOptions {}

/**
 * Additional options for the {@link ClassicLevel.iterator}, {@link ClassicLevel.keys}
 * and {@link ClassicLevel.values} methods.
 */
export interface AdditionalIteratorOptions {
  /**
   * If set to `true`, LevelDB will fill its in-memory
   * [LRU](http://en.wikipedia.org/wiki/Least_Recently_Used) cache with data
   * that was read.
   *
   * @defaultValue `false`
   */
  fillCache?: boolean | undefined

  /**
   * Limit the amount of data that the iterator will hold in memory.
   */
  highWaterMarkBytes?: number | undefined
}

/**
 * Additional options for the {@link ClassicLevel.put}, {@link ClassicLevel.del}
 * and {@link ClassicLevel.batch} methods.
 */
declare interface WriteOptions {
  /**
   * If set to `true`, LevelDB will perform a synchronous write of the data
   * although the operation will be asynchronous as far as Node.js or Electron
   * is concerned. Normally, LevelDB passes the data to the operating system
   * for writing and returns immediately. In contrast, a synchronous write will
   * use [`fsync()`](https://man7.org/linux/man-pages/man2/fsync.2.html) or
   * equivalent, so the operation will not complete until the data is actually
   * on disk. Synchronous writes are significantly slower than asynchronous
   * writes.
   *
   * @defaultValue `false`
   */
  sync?: boolean | undefined
}

/**
 * Options for the {@link ClassicLevel.put} method.
 */
export interface PutOptions<K, V> extends AbstractPutOptions<K, V>, WriteOptions {}

/**
 * Options for the {@link ClassicLevel.del} method.
 */
export interface DelOptions<K> extends AbstractDelOptions<K>, WriteOptions {}

/**
 * Options for the {@link ClassicLevel.batch} method.
 */
export interface BatchOptions<K, V> extends AbstractBatchOptions<K, V>, WriteOptions {}

/**
 * Options for the {@link ChainedBatch.write} method.
 */
export interface ChainedBatchWriteOptions extends AbstractChainedBatchWriteOptions, WriteOptions {}

export class ChainedBatch<TDatabase, KDefault, VDefault> extends AbstractChainedBatch<TDatabase, KDefault, VDefault> {
  write (): Promise<void>
  write (options: ChainedBatchWriteOptions): Promise<void>
  write (callback: NodeCallback<void>): void
  write (options: ChainedBatchWriteOptions, callback: NodeCallback<void>): void
}

/**
 * Options for the {@link ClassicLevel.approximateSize} and
 * {@link ClassicLevel.compactRange} methods.
 */
export interface StartEndOptions<K> {
  /**
   * Custom key encoding for this operation, used to encode `start` and `end`.
   */
  keyEncoding?: string | Transcoder.PartialEncoder<K> | undefined
}

// Export remaining types so that consumers don't have to guess whether they're extended
export type BatchOperation<TDatabase, K, V> = AbstractBatchOperation<TDatabase, K, V>

export type Iterator<TDatabase, K, V> = AbstractIterator<TDatabase, K, V>
export type KeyIterator<TDatabase, K> = AbstractKeyIterator<TDatabase, K>
export type ValueIterator<TDatabase, K, V> = AbstractValueIterator<TDatabase, K, V>

export type IteratorOptions<K, V> = AbstractIteratorOptions<K, V> & AdditionalIteratorOptions
export type KeyIteratorOptions<K> = AbstractKeyIteratorOptions<K> & AdditionalIteratorOptions
export type ValueIteratorOptions<K, V> = AbstractValueIteratorOptions<K, V> & AdditionalIteratorOptions
