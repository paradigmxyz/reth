import {
  AbstractLevel,
  AbstractDatabaseOptions,
  AbstractOpenOptions,
  NodeCallback
} from 'abstract-level'

/**
 * In-memory {@link AbstractLevel} database for Node.js and browsers.
 *
 * @template KDefault The default type of keys if not overridden on operations.
 * @template VDefault The default type of values if not overridden on operations.
 */
export class MemoryLevel<KDefault = string, VDefault = string>
  extends AbstractLevel<Buffer | Uint8Array | string, KDefault, VDefault> {
  /**
   * Database constructor.
   *
   * @param options Options, of which some will be forwarded to {@link open}.
   */
  constructor (options?: DatabaseOptions<KDefault, VDefault> | undefined)

  open (): Promise<void>
  open (options: OpenOptions): Promise<void>
  open (callback: NodeCallback<void>): void
  open (options: OpenOptions, callback: NodeCallback<void>): void
}

/**
 * Options for the {@link MemoryLevel} constructor.
 */
export interface DatabaseOptions<K, V> extends AbstractDatabaseOptions<K, V> {
  /**
   * How to store data internally. This affects which data types can be stored
   * non-destructively.
   *
   * The default is `'buffer'` (that means {@link Buffer}) which is non-destructive. In
   * browsers it may be preferable to use `'view'` ({@link Uint8Array}) to avoid having
   * to bundle the [`buffer`](https://github.com/feross/buffer) shim. Or if there's no
   * need to store binary data, then `'utf8'` ({@link String}).
   *
   * Regardless of the `storeEncoding`, {@link MemoryLevel} supports input that is of any
   * of the aforementioned types, but internally converts it to one type in order to
   * provide a consistent sort order.
   *
   * @defaultValue `'buffer'`
   */
  storeEncoding?: 'buffer' | 'view' | 'utf8' | undefined

  /**
   * An {@link AbstractLevel} option that has no effect on {@link MemoryLevel}.
   */
  createIfMissing?: boolean

  /**
   * An {@link AbstractLevel} option that has no effect on {@link MemoryLevel}.
   */
  errorIfExists?: boolean
}

/**
 * Options for the {@link MemoryLevel.open} method.
 */
export interface OpenOptions extends AbstractOpenOptions {
  /**
   * An {@link AbstractLevel} option that has no effect on {@link MemoryLevel}.
   */
  createIfMissing?: boolean

  /**
   * An {@link AbstractLevel} option that has no effect on {@link MemoryLevel}.
   */
  errorIfExists?: boolean
}
