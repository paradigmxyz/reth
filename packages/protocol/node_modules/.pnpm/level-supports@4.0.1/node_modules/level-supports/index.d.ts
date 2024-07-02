/**
 * Given zero or more manifest objects, returns a merged and enriched manifest object
 * that has truthy properties for each of the features listed in {@link IManifest}.
 *
 * @param manifests Partial manifest(s) to merge.
 */
export function supports (...manifests: Array<Partial<IManifest>>): IManifest

/**
 * Describes the abilities of an
 * [`abstract-level`](https://github.com/Level/abstract-level) database. Support matrices
 * for known `abstract-level` implementations can be found in
 * [`level-supports`](https://github.com/Level/supports#features).
 */
export interface IManifest {
  /**
   * Does the database have snapshot guarantees? Meaning that reads are unaffected by
   * simultaneous writes. For example, an iterator should read from a snapshot of the
   * database, created at the time `db.iterator()` was called. This means the iterator
   * will not see the data of simultaneous write operations.
   */
  snapshots: boolean

  /**
   * Does data survive after process (or environment) exit?
   */
  permanence: boolean

  /**
   * Do iterators support
   * [`seek(..)`](https://github.com/Level/abstract-level/#iteratorseektarget-options)?
   */
  seek: boolean

  /**
   * Does the database support `db.clear()`? Always true since `abstract-level@1`.
   */
  clear: boolean

  /**
   * Does the database support `db.getMany()`? Always true since `abstract-level@1`.
   */
  getMany: boolean

  /**
   * Does the database have a `keys([options])` method that returns a key iterator?
   * Always true since `abstract-level@1`.
   */
  keyIterator: boolean

  /**
   * Does the database have a `values([options])` method that returns a key iterator?
   * Always true since `abstract-level@1`.
   */
  valueIterator: boolean

  /**
   * Do iterators have a `nextv(size[, options][, callback])` method? Always true since
   * `abstract-level@1`.
   */
  iteratorNextv: boolean

  /**
   * Do iterators have a `all([options][, callback])` method? Always true since
   * `abstract-level@1`.
   */
  iteratorAll: boolean

  /**
   * Does the database have a
   * [`status`](https://github.com/Level/abstract-level/#dbstatus) property? Always true
   * since `abstract-level@1`.
   */
  status: boolean

  /**
   * Does `db.open()` and the database constructor support this option?
   */
  createIfMissing: boolean

  /**
   * Does `db.open()` and the database constructor support this option?
   */
  errorIfExists: boolean

  /**
   * Can operations like `db.put()` be called without explicitly opening the db? Like so:
   *
   * ```js
   * const db = new Level()
   * await db.put('key', 'value')
   * ```
   *
   * Always true since `abstract-level@1`.
   */
  deferredOpen: boolean

  /**
   * Do all database methods (that don't otherwise have a return value) support promises,
   * in addition to callbacks? Such that, when a callback argument is omitted, a promise
   * is returned:
   *
   * ```js
   * db.put('key', 'value', callback)
   * await db.put('key', 'value')
   * ```
   *
   * Always true since `abstract-level@1`.
   */
  promises: boolean

  /**
   * Does database have the methods `createReadStream`, `createKeyStream` and
   * `createValueStream`, following the API documented in `levelup`? For `abstract-level`
   * databases, a standalone module called
   * [`level-read-stream`](https://github.com/Level/read-stream) is available.
   */
  streams: boolean

  /**
   * Which encodings (by name) does the database support, as indicated by nested
   * properties? For example:
   *
   * ```js
   * { utf8: true, json: true }
   * ```
   */
  encodings: Record<string, boolean>

  /**
   * Which events does the database emit, as indicated by nested properties? For example:
   *
   * ```js
   * if (db.supports.events.put) {
   *   db.on('put', listener)
   * }
   * ```
   */
  events: Record<string, boolean>

  /**
   * Declares support of additional methods, that are not part of the `abstract-level`
   * interface. In the form of:
   *
   * ```js
   * {
   *   foo: true,
   *   bar: true
   * }
   * ```
   *
   * Which says the db has two methods, `foo` and `bar`. It might be used like so:
   *
   * ```js
   * if (db.supports.additionalMethods.foo) {
   *   db.foo()
   * }
   * ```
   */
  additionalMethods: Record<string, boolean>
}
