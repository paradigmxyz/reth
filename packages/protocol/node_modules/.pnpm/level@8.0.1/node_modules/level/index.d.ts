import * as AbstractLevel from 'abstract-level'
import * as ClassicLevel from 'classic-level'
import * as BrowserLevel from 'browser-level'

/**
 * Universal {@link AbstractLevel} database for Node.js and browsers.
 *
 * @template KDefault The default type of keys if not overridden on operations.
 * @template VDefault The default type of values if not overridden on operations.
 */
export class Level<KDefault = string, VDefault = string>
  extends AbstractLevel.AbstractLevel<string | Buffer | Uint8Array, KDefault, VDefault> {
  /**
   * Database constructor.
   *
   * @param location Directory path (relative or absolute) where LevelDB will store its
   * files, or in browsers, the name of the
   * [`IDBDatabase`](https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase) to be
   * opened.
   * @param options Options, of which some will be forwarded to {@link open}.
   */
  constructor (location: string, options?: DatabaseOptions<KDefault, VDefault> | undefined)

  /**
   * Location that was passed to the constructor.
   */
  get location (): string

  open (): Promise<void>
  open (options: OpenOptions): Promise<void>
  open (callback: AbstractLevel.NodeCallback<void>): void
  open (options: OpenOptions, callback: AbstractLevel.NodeCallback<void>): void

  get (key: KDefault): Promise<VDefault>
  get (key: KDefault, callback: AbstractLevel.NodeCallback<VDefault>): void
  get<K = KDefault, V = VDefault> (key: K, options: GetOptions<K, V>): Promise<V>
  get<K = KDefault, V = VDefault> (key: K, options: GetOptions<K, V>, callback: AbstractLevel.NodeCallback<V>): void

  getMany (keys: KDefault[]): Promise<VDefault[]>
  getMany (keys: KDefault[], callback: AbstractLevel.NodeCallback<VDefault[]>): void
  getMany<K = KDefault, V = VDefault> (keys: K[], options: GetManyOptions<K, V>): Promise<V[]>
  getMany<K = KDefault, V = VDefault> (keys: K[], options: GetManyOptions<K, V>, callback: AbstractLevel.NodeCallback<V[]>): void

  put (key: KDefault, value: VDefault): Promise<void>
  put (key: KDefault, value: VDefault, callback: AbstractLevel.NodeCallback<void>): void
  put<K = KDefault, V = VDefault> (key: K, value: V, options: PutOptions<K, V>): Promise<void>
  put<K = KDefault, V = VDefault> (key: K, value: V, options: PutOptions<K, V>, callback: AbstractLevel.NodeCallback<void>): void

  del (key: KDefault): Promise<void>
  del (key: KDefault, callback: AbstractLevel.NodeCallback<void>): void
  del<K = KDefault> (key: K, options: DelOptions<K>): Promise<void>
  del<K = KDefault> (key: K, options: DelOptions<K>, callback: AbstractLevel.NodeCallback<void>): void

  batch (operations: Array<BatchOperation<typeof this, KDefault, VDefault>>): Promise<void>
  batch (operations: Array<BatchOperation<typeof this, KDefault, VDefault>>, callback: AbstractLevel.NodeCallback<void>): void
  batch<K = KDefault, V = VDefault> (operations: Array<BatchOperation<typeof this, K, V>>, options: BatchOptions<K, V>): Promise<void>
  batch<K = KDefault, V = VDefault> (operations: Array<BatchOperation<typeof this, K, V>>, options: BatchOptions<K, V>, callback: AbstractLevel.NodeCallback<void>): void
  batch (): ChainedBatch<typeof this, KDefault, VDefault>

  iterator (): Iterator<typeof this, KDefault, VDefault>
  iterator<K = KDefault, V = VDefault> (options: IteratorOptions<K, V>): Iterator<typeof this, K, V>

  keys (): KeyIterator<typeof this, KDefault>
  keys<K = KDefault> (options: KeyIteratorOptions<K>): KeyIterator<typeof this, K>

  values (): ValueIterator<typeof this, KDefault, VDefault>
  values<K = KDefault, V = VDefault> (options: ValueIteratorOptions<K, V>): ValueIterator<typeof this, K, V>
}

export type DatabaseOptions<K, V> = ClassicLevel.DatabaseOptions<K, V> & BrowserLevel.DatabaseOptions<K, V>
export type OpenOptions = ClassicLevel.OpenOptions & BrowserLevel.OpenOptions
export type GetOptions<K, V> = ClassicLevel.GetOptions<K, V> & BrowserLevel.GetOptions<K, V>
export type GetManyOptions<K, V> = ClassicLevel.GetManyOptions<K, V> & BrowserLevel.GetManyOptions<K, V>
export type PutOptions<K, V> = ClassicLevel.PutOptions<K, V> & BrowserLevel.PutOptions<K, V>
export type DelOptions<K> = ClassicLevel.DelOptions<K> & BrowserLevel.DelOptions<K>

export type BatchOptions<K, V> = ClassicLevel.BatchOptions<K, V> & BrowserLevel.BatchOptions<K, V>
export type BatchOperation<TDatabase, K, V> = ClassicLevel.BatchOperation<TDatabase, K, V> & BrowserLevel.BatchOperation<TDatabase, K, V>
export type ChainedBatch<TDatabase, K, V> = ClassicLevel.ChainedBatch<TDatabase, K, V> & BrowserLevel.ChainedBatch<TDatabase, K, V>

export type Iterator<TDatabase, K, V> = ClassicLevel.Iterator<TDatabase, K, V> & BrowserLevel.Iterator<TDatabase, K, V>
export type KeyIterator<TDatabase, K> = ClassicLevel.KeyIterator<TDatabase, K> & BrowserLevel.KeyIterator<TDatabase, K>
export type ValueIterator<TDatabase, K, V> = ClassicLevel.ValueIterator<TDatabase, K, V> & BrowserLevel.ValueIterator<TDatabase, K, V>

export type IteratorOptions<K, V> = ClassicLevel.IteratorOptions<K, V> & BrowserLevel.IteratorOptions<K, V>
export type KeyIteratorOptions<K> = ClassicLevel.KeyIteratorOptions<K> & BrowserLevel.KeyIteratorOptions<K>
export type ValueIteratorOptions<K, V> = ClassicLevel.ValueIteratorOptions<K, V> & BrowserLevel.ValueIteratorOptions<K, V>
