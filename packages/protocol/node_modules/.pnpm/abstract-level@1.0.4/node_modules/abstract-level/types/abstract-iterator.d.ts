import * as Transcoder from 'level-transcoder'
import { RangeOptions, NodeCallback } from './interfaces'

export interface AbstractIteratorOptions<K, V> extends RangeOptions<K> {
  /**
   * Whether to return the key of each entry. Defaults to `true`. If set to `false`,
   * the iterator will yield keys that are `undefined`.
   */
  keys?: boolean | undefined

  /**
   * Whether to return the value of each entry. Defaults to `true`. If set to
   * `false`, the iterator will yield values that are `undefined`.
   */
  values?: boolean | undefined

  /**
   * Custom key encoding for this iterator, used to encode range options, to encode
   * {@link AbstractIterator.seek} targets and to decode keys.
   */
  keyEncoding?: string | Transcoder.PartialEncoding<K> | undefined

  /**
   * Custom value encoding for this iterator, used to decode values.
   */
  valueEncoding?: string | Transcoder.PartialDecoder<V> | undefined
}

export interface AbstractKeyIteratorOptions<K> extends RangeOptions<K> {
  /**
   * Custom key encoding for this iterator, used to encode range options, to encode
   * {@link AbstractKeyIterator.seek} targets and to decode keys.
   */
  keyEncoding?: string | Transcoder.PartialEncoding<K> | undefined
}

export interface AbstractValueIteratorOptions<K, V> extends RangeOptions<K> {
  /**
   * Custom key encoding for this iterator, used to encode range options and
   * {@link AbstractValueIterator.seek} targets.
   */
  keyEncoding?: string | Transcoder.PartialEncoding<K> | undefined

  /**
   * Custom value encoding for this iterator, used to decode values.
   */
  valueEncoding?: string | Transcoder.PartialDecoder<V> | undefined
}

/**
 * @template TDatabase Type of the database that created this iterator.
 * @template T Type of items yielded. Items can be entries, keys or values.
 */
declare class CommonIterator<TDatabase, T> {
  /**
   * A reference to the database that created this iterator.
   */
  db: TDatabase

  /**
   * Read-only getter that indicates how many items have been yielded so far (by any
   * method) excluding calls that errored or yielded `undefined`.
   */
  get count (): number

  /**
   * Read-only getter that reflects the `limit` that was set in options. Greater than or
   * equal to zero. Equals {@link Infinity} if no limit.
   */
  get limit (): number

  [Symbol.asyncIterator] (): AsyncGenerator<T, void, unknown>

  /**
   * Free up underlying resources. Not necessary to call if [`for await...of`][1] or
   * `all()` is used.
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   */
  close (): Promise<void>
  close (callback: NodeCallback<void>): void
}

export class AbstractIterator<TDatabase, K, V> extends CommonIterator<TDatabase, [K, V]> {
  constructor (db: TDatabase, options: AbstractIteratorOptions<K, V>)

  /**
   * Advance to the next entry and yield that entry. When possible, prefer to use
   * [`for await...of`][1] instead.
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   */
  next (): Promise<[K, V] | undefined>
  next (callback: NextCallback<K, V>): void

  /**
   * Advance repeatedly and get at most {@link size} amount of entries in a single call.
   * Can be faster than repeated {@link next()} calls. The natural end of the iterator
   * will be signaled by yielding an empty array.
   *
   * @param size Get at most this many entries. Has a soft minimum of 1.
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  nextv (size: number, options: {}, callback: NodeCallback<Array<[K, V]>>): void
  nextv (size: number, callback: NodeCallback<Array<[K, V]>>): void
  nextv (size: number, options: {}): Promise<Array<[K, V]>>
  nextv (size: number): Promise<Array<[K, V]>>

  /**
   * Advance repeatedly and get all (remaining) entries as an array, automatically
   * closing the iterator. Assumes that those entries fit in memory. If that's not the
   * case, instead use {@link next()}, {@link nextv()} or [`for await...of`][1].
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   *
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  all (options: {}, callback: NodeCallback<Array<[K, V]>>): void
  all (callback: NodeCallback<Array<[K, V]>>): void
  all (options: {}): Promise<Array<[K, V]>>
  all (): Promise<Array<[K, V]>>

  /**
   * Seek to the key closest to {@link target}. Subsequent calls to {@link next()},
   * {@link nextv()} or {@link all()} (including implicit calls in a `for await...of`
   * loop) will yield entries with keys equal to or larger than {@link target}, or equal
   * to or smaller than {@link target} if the {@link AbstractIteratorOptions.reverse}
   * option was true.
   */
  seek (target: K): void
  seek<TTarget = K> (target: TTarget, options: AbstractSeekOptions<TTarget>): void
}

export class AbstractKeyIterator<TDatabase, K> extends CommonIterator<TDatabase, K> {
  constructor (db: TDatabase, options: AbstractKeyIteratorOptions<K>)

  /**
   * Advance to the next key and yield that key. When possible, prefer to use
   * [`for await...of`][1] instead.
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   */
  next (): Promise<K | undefined>
  next (callback: NodeCallback<K>): void

  /**
   * Advance repeatedly and get at most {@link size} amount of keys in a single call. Can
   * be faster than repeated {@link next()} calls. The natural end of the iterator will
   * be signaled by yielding an empty array.
   *
   * @param size Get at most this many keys. Has a soft minimum of 1.
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  nextv (size: number, options: {}, callback: NodeCallback<[K]>): void
  nextv (size: number, callback: NodeCallback<[K]>): void
  nextv (size: number, options: {}): Promise<[K]>
  nextv (size: number): Promise<[K]>

  /**
   * Advance repeatedly and get all (remaining) keys as an array, automatically closing
   * the iterator. Assumes that those keys fit in memory. If that's not the case, instead
   * use {@link next()}, {@link nextv()} or [`for await...of`][1].
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   *
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  all (options: {}, callback: NodeCallback<[K]>): void
  all (callback: NodeCallback<[K]>): void
  all (options: {}): Promise<[K]>
  all (): Promise<[K]>

  /**
   * Seek to the key closest to {@link target}. Subsequent calls to {@link next()},
   * {@link nextv()} or {@link all()} (including implicit calls in a `for await...of`
   * loop) will yield keys equal to or larger than {@link target}, or equal to or smaller
   * than {@link target} if the {@link AbstractKeyIteratorOptions.reverse} option was
   * true.
   */
  seek (target: K): void
  seek<TTarget = K> (target: TTarget, options: AbstractSeekOptions<TTarget>): void
}

export class AbstractValueIterator<TDatabase, K, V> extends CommonIterator<TDatabase, V> {
  constructor (db: TDatabase, options: AbstractValueIteratorOptions<K, V>)

  /**
   * Advance to the next value and yield that value. When possible, prefer
   * to use [`for await...of`][1] instead.
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   */
  next (): Promise<V | undefined>
  next (callback: NodeCallback<V>): void

  /**
   * Advance repeatedly and get at most {@link size} amount of values in a single call.
   * Can be faster than repeated {@link next()} calls. The natural end of the iterator
   * will be signaled by yielding an empty array.
   *
   * @param size Get at most this many values. Has a soft minimum of 1.
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  nextv (size: number, options: {}, callback: NodeCallback<[V]>): void
  nextv (size: number, callback: NodeCallback<[V]>): void
  nextv (size: number, options: {}): Promise<[V]>
  nextv (size: number): Promise<[V]>

  /**
   * Advance repeatedly and get all (remaining) values as an array, automatically closing
   * the iterator. Assumes that those values fit in memory. If that's not the case,
   * instead use {@link next()}, {@link nextv()} or [`for await...of`][1].
   *
   * [1]: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of
   *
   * @param options Options (none at the moment, reserved for future use).
   * @param callback Error-first callback. If none is provided, a promise is returned.
   */
  all (options: {}, callback: NodeCallback<[V]>): void
  all (callback: NodeCallback<[V]>): void
  all (options: {}): Promise<[V]>
  all (): Promise<[V]>

  /**
   * Seek to the key closest to {@link target}. Subsequent calls to {@link next()},
   * {@link nextv()} or {@link all()} (including implicit calls in a `for await...of`
   * loop) will yield the values of keys equal to or larger than {@link target}, or equal
   * to or smaller than {@link target} if the {@link AbstractValueIteratorOptions.reverse}
   * option was true.
   */
  seek (target: K): void
  seek<TTarget = K> (target: TTarget, options: AbstractSeekOptions<TTarget>): void
}

/**
 * Options for the {@link AbstractIterator.seek} method.
 */
export interface AbstractSeekOptions<K> {
  /**
   * Custom key encoding, used to encode the `target`. By default the keyEncoding option
   * of the iterator is used, or (if that wasn't set) the keyEncoding of the database.
   */
  keyEncoding?: string | Transcoder.PartialEncoder<K> | undefined
}

declare type NextCallback<K, V> =
  (err: Error | undefined | null, key?: K | undefined, value?: V | undefined) => void
