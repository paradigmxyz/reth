# abstract-level

**Abstract class for a lexicographically sorted key-value database.** The successor to [`abstract-leveldown`](https://github.com/Level/abstract-leveldown) with builtin encodings, sublevels, events, promises and support of Uint8Array. If you are upgrading please see [`UPGRADING.md`](UPGRADING.md).

> :pushpin: Which module should I use? What happened to `levelup`? Head over to the [FAQ](https://github.com/Level/community#faq).

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/abstract-level.svg)](https://www.npmjs.com/package/abstract-level)
[![Node version](https://img.shields.io/node/v/abstract-level.svg)](https://www.npmjs.com/package/abstract-level)
[![Test](https://img.shields.io/github/workflow/status/Level/abstract-level/Test?label=test)](https://github.com/Level/abstract-level/actions/workflows/test.yml)
[![Browsers](https://img.shields.io/github/workflow/status/Level/abstract-level/Browsers?label=browsers)](https://github.com/Level/abstract-level/actions/workflows/browsers.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/abstract-level?label=\&logo=codecov\&logoColor=fff)](https://codecov.io/gh/Level/abstract-level)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript\&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective\&logoColor=fff)](https://opencollective.com/level)

## Table of Contents

<details><summary>Click to expand</summary>

- [Usage](#usage)
- [Supported Platforms](#supported-platforms)
- [Public API For Consumers](#public-api-for-consumers)
  - [`db = new Constructor(...[, options])`](#db--new-constructor-options)
  - [`db.status`](#dbstatus)
  - [`db.open([options][, callback])`](#dbopenoptions-callback)
  - [`db.close([callback])`](#dbclosecallback)
  - [`db.supports`](#dbsupports)
  - [`db.get(key[, options][, callback])`](#dbgetkey-options-callback)
  - [`db.getMany(keys[, options][, callback])`](#dbgetmanykeys-options-callback)
  - [`db.put(key, value[, options][, callback])`](#dbputkey-value-options-callback)
  - [`db.del(key[, options][, callback])`](#dbdelkey-options-callback)
  - [`db.batch(operations[, options][, callback])`](#dbbatchoperations-options-callback)
  - [`chainedBatch = db.batch()`](#chainedbatch--dbbatch)
  - [`iterator = db.iterator([options])`](#iterator--dbiteratoroptions)
  - [`keyIterator = db.keys([options])`](#keyiterator--dbkeysoptions)
  - [`valueIterator = db.values([options])`](#valueiterator--dbvaluesoptions)
  - [`db.clear([options][, callback])`](#dbclearoptions-callback)
  - [`sublevel = db.sublevel(name[, options])`](#sublevel--dbsublevelname-options)
  - [`encoding = db.keyEncoding([encoding])`](#encoding--dbkeyencodingencoding)
  - [`encoding = db.valueEncoding([encoding])`](#encoding--dbvalueencodingencoding)
  - [`key = db.prefixKey(key, keyFormat)`](#key--dbprefixkeykey-keyformat)
  - [`db.defer(fn)`](#dbdeferfn)
  - [`chainedBatch`](#chainedbatch)
    - [`chainedBatch.put(key, value[, options])`](#chainedbatchputkey-value-options)
    - [`chainedBatch.del(key[, options])`](#chainedbatchdelkey-options)
    - [`chainedBatch.clear()`](#chainedbatchclear)
    - [`chainedBatch.write([options][, callback])`](#chainedbatchwriteoptions-callback)
    - [`chainedBatch.close([callback])`](#chainedbatchclosecallback)
    - [`chainedBatch.length`](#chainedbatchlength)
    - [`chainedBatch.db`](#chainedbatchdb)
  - [`iterator`](#iterator)
    - [`for await...of iterator`](#for-awaitof-iterator)
    - [`iterator.next([callback])`](#iteratornextcallback)
    - [`iterator.nextv(size[, options][, callback])`](#iteratornextvsize-options-callback)
    - [`iterator.all([options][, callback])`](#iteratoralloptions-callback)
    - [`iterator.seek(target[, options])`](#iteratorseektarget-options)
    - [`iterator.close([callback])`](#iteratorclosecallback)
    - [`iterator.db`](#iteratordb)
    - [`iterator.count`](#iteratorcount)
    - [`iterator.limit`](#iteratorlimit)
  - [`keyIterator`](#keyiterator)
  - [`valueIterator`](#valueiterator)
  - [`sublevel`](#sublevel)
    - [`sublevel.prefix`](#sublevelprefix)
    - [`sublevel.db`](#subleveldb)
  - [Encodings](#encodings)
  - [Events](#events)
  - [Errors](#errors)
    - [`LEVEL_NOT_FOUND`](#level_not_found)
    - [`LEVEL_DATABASE_NOT_OPEN`](#level_database_not_open)
    - [`LEVEL_DATABASE_NOT_CLOSED`](#level_database_not_closed)
    - [`LEVEL_ITERATOR_NOT_OPEN`](#level_iterator_not_open)
    - [`LEVEL_ITERATOR_BUSY`](#level_iterator_busy)
    - [`LEVEL_BATCH_NOT_OPEN`](#level_batch_not_open)
    - [`LEVEL_ENCODING_NOT_FOUND`](#level_encoding_not_found)
    - [`LEVEL_ENCODING_NOT_SUPPORTED`](#level_encoding_not_supported)
    - [`LEVEL_DECODE_ERROR`](#level_decode_error)
    - [`LEVEL_INVALID_KEY`](#level_invalid_key)
    - [`LEVEL_INVALID_VALUE`](#level_invalid_value)
    - [`LEVEL_CORRUPTION`](#level_corruption)
    - [`LEVEL_IO_ERROR`](#level_io_error)
    - [`LEVEL_INVALID_PREFIX`](#level_invalid_prefix)
    - [`LEVEL_NOT_SUPPORTED`](#level_not_supported)
    - [`LEVEL_LEGACY`](#level_legacy)
    - [`LEVEL_LOCKED`](#level_locked)
    - [`LEVEL_READONLY`](#level_readonly)
    - [`LEVEL_CONNECTION_LOST`](#level_connection_lost)
    - [`LEVEL_REMOTE_ERROR`](#level_remote_error)
  - [Shared Access](#shared-access)
- [Private API For Implementors](#private-api-for-implementors)
  - [Example](#example)
  - [`db = AbstractLevel(manifest[, options])`](#db--abstractlevelmanifest-options)
  - [`db._open(options, callback)`](#db_openoptions-callback)
  - [`db._close(callback)`](#db_closecallback)
  - [`db._get(key, options, callback)`](#db_getkey-options-callback)
  - [`db._getMany(keys, options, callback)`](#db_getmanykeys-options-callback)
  - [`db._put(key, value, options, callback)`](#db_putkey-value-options-callback)
  - [`db._del(key, options, callback)`](#db_delkey-options-callback)
  - [`db._batch(operations, options, callback)`](#db_batchoperations-options-callback)
  - [`db._chainedBatch()`](#db_chainedbatch)
  - [`db._iterator(options)`](#db_iteratoroptions)
  - [`db._keys(options)`](#db_keysoptions)
  - [`db._values(options)`](#db_valuesoptions)
  - [`db._clear(options, callback)`](#db_clearoptions-callback)
  - [`sublevel = db._sublevel(name, options)`](#sublevel--db_sublevelname-options)
  - [`iterator = AbstractIterator(db, options)`](#iterator--abstractiteratordb-options)
    - [`iterator._next(callback)`](#iterator_nextcallback)
    - [`iterator._nextv(size, options, callback)`](#iterator_nextvsize-options-callback)
    - [`iterator._all(options, callback)`](#iterator_alloptions-callback)
    - [`iterator._seek(target, options)`](#iterator_seektarget-options)
    - [`iterator._close(callback)`](#iterator_closecallback)
  - [`keyIterator = AbstractKeyIterator(db, options)`](#keyiterator--abstractkeyiteratordb-options)
  - [`valueIterator = AbstractValueIterator(db, options)`](#valueiterator--abstractvalueiteratordb-options)
  - [`chainedBatch = AbstractChainedBatch(db)`](#chainedbatch--abstractchainedbatchdb)
    - [`chainedBatch._put(key, value, options)`](#chainedbatch_putkey-value-options)
    - [`chainedBatch._del(key, options)`](#chainedbatch_delkey-options)
    - [`chainedBatch._clear()`](#chainedbatch_clear)
    - [`chainedBatch._write(options, callback)`](#chainedbatch_writeoptions-callback)
    - [`chainedBatch._close(callback)`](#chainedbatch_closecallback)
- [Test Suite](#test-suite)
  - [Excluding tests](#excluding-tests)
  - [Reusing `testCommon`](#reusing-testcommon)
- [Spread The Word](#spread-the-word)
- [Install](#install)
- [Contributing](#contributing)
- [Donate](#donate)
- [License](#license)

</details>

## Usage

Usage of a typical implementation looks as follows.

```js
// Create a database
const db = new Level('./db', { valueEncoding: 'json' })

// Add an entry with key 'a' and value 1
await db.put('a', 1)

// Add multiple entries
await db.batch([{ type: 'put', key: 'b', value: 2 }])

// Get value of key 'a': 1
const value = await db.get('a')

// Iterate entries with keys that are greater than 'a'
for await (const [key, value] of db.iterator({ gt: 'a' })) {
  console.log(value) // 2
}
```

All asynchronous methods also support callbacks.

<details><summary>Callback example</summary>

```js
db.put('a', { x: 123 }, function (err) {
  if (err) throw err

  db.get('a', function (err, value) {
    console.log(value) // { x: 123 }
  })
})
```

</details>

Usage from TypeScript requires generic type parameters.

<details><summary>TypeScript example</summary>

```ts
// Specify types of keys and values (any, in the case of json).
// The generic type parameters default to Level<string, string>.
const db = new Level<string, any>('./db', { valueEncoding: 'json' })

// All relevant methods then use those types
await db.put('a', { x: 123 })

// Specify different types when overriding encoding per operation
await db.get<string, string>('a', { valueEncoding: 'utf8' })

// Though in some cases TypeScript can infer them
await db.get('a', { valueEncoding: db.valueEncoding('utf8') })

// It works the same for sublevels
const abc = db.sublevel('abc')
const xyz = db.sublevel<string, any>('xyz', { valueEncoding: 'json' })
```

</details>

## Supported Platforms

We aim to support Active LTS and Current Node.js releases, as well as evergreen browsers that are based on Chromium, Firefox or Webkit. Supported runtime environments may differ per implementation.

## Public API For Consumers

This module has a public API for consumers of a database and a [private API](#private-api-for-implementors) for concrete implementations. The public API, as documented in this section, offers a simple yet rich interface that is common between all implementations. Implementations may have additional options or methods. TypeScript [type declarations](https://www.typescriptlang.org/docs/handbook/2/type-declarations.html) are [included](./index.d.ts) (and exported for reuse) only for the public API.

An `abstract-level` database is at its core a [key-value database](https://en.wikipedia.org/wiki/Key%E2%80%93value_database). A key-value pair is referred to as an _entry_ here and typically returned as an array, comparable to [`Object.entries()`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Object/entries).

### `db = new Constructor(...[, options])`

Creating a database is done by calling a class constructor. Implementations export a class that extends the [`AbstractLevel`](./abstract-level.js) class and has its own constructor with an implementation-specific signature. All constructors should have an `options` argument as the last. Typically, constructors take a `location` as their first argument, pointing to where the data will be stored. That may be a file path, URL, something else or none at all, since not all implementations are disk-based or persistent. Others take another database rather than a location as their first argument.

The optional `options` object may contain:

- `keyEncoding` (string or object, default `'utf8'`): encoding to use for keys
- `valueEncoding` (string or object, default `'utf8'`): encoding to use for values.

See [Encodings](#encodings) for a full description of these options. Other `options` (except `passive`) are forwarded to `db.open()` which is automatically called in a next tick after the constructor returns. Any read & write operations are queued internally until the database has finished opening. If opening fails, those queued operations will yield errors.

### `db.status`

Read-only getter that returns a string reflecting the current state of the database:

- `'opening'` - waiting for the database to be opened
- `'open'` - successfully opened the database
- `'closing'` - waiting for the database to be closed
- `'closed'` - successfully closed the database.

### `db.open([options][, callback])`

Open the database. The `callback` function will be called with no arguments when successfully opened, or with a single error argument if opening failed. If no callback is provided, a promise is returned. Options passed to `open()` take precedence over options passed to the database constructor. Not all implementations support the `createIfMissing` and `errorIfExists` options (notably [`memory-level`](https://github.com/Level/memory-level) and [`browser-level`](https://github.com/Level/browser-level)) and will indicate so via `db.supports.createIfMissing` and `db.supports.errorIfExists`.

The optional `options` object may contain:

- `createIfMissing` (boolean, default: `true`): If `true`, create an empty database if one doesn't already exist. If `false` and the database doesn't exist, opening will fail.
- `errorIfExists` (boolean, default: `false`): If `true` and the database already exists, opening will fail.
- `passive` (boolean, default: `false`): Wait for, but do not initiate, opening of the database.

It's generally not necessary to call `open()` because it's automatically called by the database constructor. It may however be useful to capture an error from failure to open, that would otherwise not surface until another method like `db.get()` is called. It's also possible to reopen the database after it has been closed with [`close()`](#dbclosecallback). Once `open()` has then been called, any read & write operations will again be queued internally until opening has finished.

The `open()` and `close()` methods are idempotent. If the database is already open, the `callback` will be called in a next tick. If opening is already in progress, the `callback` will be called when that has finished. If closing is in progress, the database will be reopened once closing has finished. Likewise, if `close()` is called after `open()`, the database will be closed once opening has finished and the prior `open()` call will receive an error.

### `db.close([callback])`

Close the database. The `callback` function will be called with no arguments if closing succeeded or with a single `error` argument if closing failed. If no callback is provided, a promise is returned.

A database may have associated resources like file handles and locks. When the database is no longer needed (for the remainder of a program) it's recommended to call `db.close()` to free up resources.

After `db.close()` has been called, no further read & write operations are allowed unless and until `db.open()` is called again. For example, `db.get(key)` will yield an error with code [`LEVEL_DATABASE_NOT_OPEN`](#errors). Any unclosed iterators or chained batches will be closed by `db.close()` and can then no longer be used even when `db.open()` is called again.

### `db.supports`

A [manifest](https://github.com/Level/supports) describing the features supported by this database. Might be used like so:

```js
if (!db.supports.permanence) {
  throw new Error('Persistent storage is required')
}
```

### `db.get(key[, options][, callback])`

Get a value from the database by `key`. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `valueEncoding`: custom value encoding for this operation, used to decode the value.

The `callback` function will be called with an error if the operation failed. If the key was not found, the error will have code [`LEVEL_NOT_FOUND`](#errors). If successful the first argument will be `null` and the second argument will be the value. If no callback is provided, a promise is returned.

If the database indicates support of snapshots via `db.supports.snapshots` then `db.get()` _should_ read from a snapshot of the database, created at the time `db.get()` was called. This means it should not see the data of simultaneous write operations. However, this is currently not verified by the [abstract test suite](#test-suite).

### `db.getMany(keys[, options][, callback])`

Get multiple values from the database by an array of `keys`. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `keys`.
- `valueEncoding`: custom value encoding for this operation, used to decode values.

The `callback` function will be called with an error if the operation failed. If successful the first argument will be `null` and the second argument will be an array of values with the same order as `keys`. If a key was not found, the relevant value will be `undefined`. If no callback is provided, a promise is returned.

If the database indicates support of snapshots via `db.supports.snapshots` then `db.getMany()` _should_ read from a snapshot of the database, created at the time `db.getMany()` was called. This means it should not see the data of simultaneous write operations. However, this is currently not verified by the [abstract test suite](#test-suite).

### `db.put(key, value[, options][, callback])`

Add a new entry or overwrite an existing entry. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `valueEncoding`: custom value encoding for this operation, used to encode the `value`.

The `callback` function will be called with no arguments if the operation was successful or with an error if it failed. If no callback is provided, a promise is returned.

### `db.del(key[, options][, callback])`

Delete an entry by `key`. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.

The `callback` function will be called with no arguments if the operation was successful or with an error if it failed. If no callback is provided, a promise is returned.

### `db.batch(operations[, options][, callback])`

Perform multiple _put_ and/or _del_ operations in bulk. The `operations` argument must be an array containing a list of operations to be executed sequentially, although as a whole they are performed as an atomic operation.

Each operation must be an object with at least a `type` property set to either `'put'` or `'del'`. If the `type` is `'put'`, the operation must have `key` and `value` properties. It may optionally have `keyEncoding` and / or `valueEncoding` properties to encode keys or values with a custom encoding for just that operation. If the `type` is `'del'`, the operation must have a `key` property and may optionally have a `keyEncoding` property.

An operation of either type may also have a `sublevel` property, to prefix the key of the operation with the prefix of that sublevel. This allows atomically committing data to multiple sublevels. Keys and values will be encoded by the sublevel, to the same effect as a `sublevel.batch(..)` call. In the following example, the first `value` will be encoded with `'json'` rather than the default encoding of `db`:

```js
const people = db.sublevel('people', { valueEncoding: 'json' })
const nameIndex = db.sublevel('names')

await db.batch([{
  type: 'put',
  sublevel: people,
  key: '123',
  value: {
    name: 'Alice'
  }
}, {
  type: 'put',
  sublevel: nameIndex,
  key: 'Alice',
  value: '123'
}])
```

The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this batch, used to encode keys.
- `valueEncoding`: custom value encoding for this batch, used to encode values.

Encoding properties on individual operations take precedence. In the following example, the first value will be encoded with the `'utf8'` encoding and the second with `'json'`.

```js
await db.batch([
  { type: 'put', key: 'a', value: 'foo' },
  { type: 'put', key: 'b', value: 123, valueEncoding: 'json' }
], { valueEncoding: 'utf8' })
```

The `callback` function will be called with no arguments if the batch was successful or with an error if it failed. If no callback is provided, a promise is returned.

### `chainedBatch = db.batch()`

Create a [chained batch](#chainedbatch), when `batch()` is called with zero arguments. A chained batch can be used to build and eventually commit an atomic batch of operations. Depending on how it's used, it is possible to obtain greater performance with this form of `batch()`. On several implementations however, it is just sugar.

```js
await db.batch()
  .del('bob')
  .put('alice', 361)
  .put('kim', 220)
  .write()
```

### `iterator = db.iterator([options])`

Create an [iterator](#iterator). The optional `options` object may contain the following _range options_ to control the range of entries to be iterated:

- `gt` (greater than) or `gte` (greater than or equal): define the lower bound of the range to be iterated. Only entries where the key is greater than (or equal to) this option will be included in the range. When `reverse` is true the order will be reversed, but the entries iterated will be the same.
- `lt` (less than) or `lte` (less than or equal): define the higher bound of the range to be iterated. Only entries where the key is less than (or equal to) this option will be included in the range. When `reverse` is true the order will be reversed, but the entries iterated will be the same.
- `reverse` (boolean, default: `false`): iterate entries in reverse order. Beware that a reverse seek can be slower than a forward seek.
- `limit` (number, default: `Infinity`): limit the number of entries yielded. This number represents a _maximum_ number of entries and will not be reached if the end of the range is reached first. A value of `Infinity` or `-1` means there is no limit. When `reverse` is true the entries with the highest keys will be returned instead of the lowest keys.

The `gte` and `lte` range options take precedence over `gt` and `lt` respectively. If no range options are provided, the iterator will visit all entries of the database, starting at the lowest key and ending at the highest key (unless `reverse` is true). In addition to range options, the `options` object may contain:

- `keys` (boolean, default: `true`): whether to return the key of each entry. If set to `false`, the iterator will yield keys that are `undefined`. Prefer to use `db.keys()` instead.
- `values` (boolean, default: `true`): whether to return the value of each entry. If set to `false`, the iterator will yield values that are `undefined`. Prefer to use `db.values()` instead.
- `keyEncoding`: custom key encoding for this iterator, used to encode range options, to encode `seek()` targets and to decode keys.
- `valueEncoding`: custom value encoding for this iterator, used to decode values.

Lastly, an implementation is free to add its own options.

> :pushpin: To instead consume data using streams, see [`level-read-stream`](https://github.com/Level/read-stream) and [`level-web-stream`](https://github.com/Level/web-stream).

### `keyIterator = db.keys([options])`

Create a [key iterator](#keyiterator), having the same interface as `db.iterator()` except that it yields keys instead of entries. If only keys are needed, using `db.keys()` may increase performance because values won't have to fetched, copied or decoded. Options are the same as for `db.iterator()` except that `db.keys()` does not take `keys`, `values` and `valueEncoding` options.

```js
// Iterate lazily
for await (const key of db.keys({ gt: 'a' })) {
  console.log(key)
}

// Get all at once. Setting a limit is recommended.
const keys = await db.keys({ gt: 'a', limit: 10 }).all()
```

### `valueIterator = db.values([options])`

Create a [value iterator](#valueiterator), having the same interface as `db.iterator()` except that it yields values instead of entries. If only values are needed, using `db.values()` may increase performance because keys won't have to fetched, copied or decoded. Options are the same as for `db.iterator()` except that `db.values()` does not take `keys` and `values` options. Note that it _does_ take a `keyEncoding` option, relevant for the encoding of range options.

```js
// Iterate lazily
for await (const value of db.values({ gt: 'a' })) {
  console.log(value)
}

// Get all at once. Setting a limit is recommended.
const values = await db.values({ gt: 'a', limit: 10 }).all()
```

### `db.clear([options][, callback])`

Delete all entries or a range. Not guaranteed to be atomic. Accepts the following options (with the same rules as on iterators):

- `gt` (greater than) or `gte` (greater than or equal): define the lower bound of the range to be deleted. Only entries where the key is greater than (or equal to) this option will be included in the range. When `reverse` is true the order will be reversed, but the entries deleted will be the same.
- `lt` (less than) or `lte` (less than or equal): define the higher bound of the range to be deleted. Only entries where the key is less than (or equal to) this option will be included in the range. When `reverse` is true the order will be reversed, but the entries deleted will be the same.
- `reverse` (boolean, default: `false`): delete entries in reverse order. Only effective in combination with `limit`, to delete the last N entries.
- `limit` (number, default: `Infinity`): limit the number of entries to be deleted. This number represents a _maximum_ number of entries and will not be reached if the end of the range is reached first. A value of `Infinity` or `-1` means there is no limit. When `reverse` is true the entries with the highest keys will be deleted instead of the lowest keys.
- `keyEncoding`: custom key encoding for this operation, used to encode range options.

The `gte` and `lte` range options take precedence over `gt` and `lt` respectively. If no options are provided, all entries will be deleted. The `callback` function will be called with no arguments if the operation was successful or with an error if it failed. If no callback is provided, a promise is returned.

### `sublevel = db.sublevel(name[, options])`

Create a [sublevel](#sublevel) that has the same interface as `db` (except for additional, implementation-specific methods) and prefixes the keys of operations before passing them on to `db`. The `name` argument is required and must be a string.

```js
const example = db.sublevel('example')

await example.put('hello', 'world')
await db.put('a', '1')

// Prints ['hello', 'world']
for await (const [key, value] of example.iterator()) {
  console.log([key, value])
}
```

Sublevels effectively separate a database into sections. Think SQL tables, but evented, ranged and realtime! Each sublevel is an `AbstractLevel` instance with its own keyspace, [events](https://github.com/Level/abstract-level#events) and [encodings](https://github.com/Level/abstract-level#encodings). For example, it's possible to have one sublevel with `'buffer'` keys and another with `'utf8'` keys. The same goes for values. Like so:

```js
db.sublevel('one', { valueEncoding: 'json' })
db.sublevel('two', { keyEncoding: 'buffer' })
```

An own keyspace means that `sublevel.iterator()` only includes entries of that sublevel, `sublevel.clear()` will only delete entries of that sublevel, and so forth. Range options get prefixed too.

Fully qualified keys (as seen from the parent database) take the form of `prefix + key` where `prefix` is `separator + name + separator`. If `name` is empty, the effective prefix is two separators. Sublevels can be nested: if `db` is itself a sublevel then the effective prefix is a combined prefix, e.g. `'!one!!two!'`. Note that a parent database will see its own keys as well as keys of any nested sublevels:

```js
// Prints ['!example!hello', 'world'] and ['a', '1']
for await (const [key, value] of db.iterator()) {
  console.log([key, value])
}
```

> :pushpin: The key structure is equal to that of [`subleveldown`](https://github.com/Level/subleveldown) which offered sublevels before they were built-in to `abstract-level`. This means that an `abstract-level` sublevel can read sublevels previously created with (and populated by) `subleveldown`.

Internally, sublevels operate on keys that are either a string, Buffer or Uint8Array, depending on parent database and choice of encoding. Which is to say: binary keys are fully supported. The `name` must however always be a string and can only contain ASCII characters.

The optional `options` object may contain:

- `separator` (string, default: `'!'`): Character for separating sublevel names from user keys and each other. Must sort before characters used in `name`. An error will be thrown if that's not the case.
- `keyEncoding` (string or object, default `'utf8'`): encoding to use for keys
- `valueEncoding` (string or object, default `'utf8'`): encoding to use for values.

The `keyEncoding` and `valueEncoding` options are forwarded to the `AbstractLevel` constructor and work the same, as if a new, separate database was created. They default to `'utf8'` regardless of the encodings configured on `db`. Other options are forwarded too but `abstract-level` has no relevant options at the time of writing. For example, setting the `createIfMissing` option will have no effect. Why is that?

Like regular databases, sublevels open themselves but they do not affect the state of the parent database. This means a sublevel can be individually closed and (re)opened. If the sublevel is created while the parent database is opening, it will wait for that to finish. If the parent database is closed, then opening the sublevel will fail and subsequent operations on the sublevel will yield errors with code [`LEVEL_DATABASE_NOT_OPEN`](#errors).

### `encoding = db.keyEncoding([encoding])`

Returns the given `encoding` argument as a normalized encoding object that follows the [`level-transcoder`](https://github.com/Level/transcoder) encoding interface. See [Encodings](#encodings) for an introduction. The `encoding` argument may be:

- A string to select a known encoding by its name
- An object that follows one of the following interfaces: [`level-transcoder`](https://github.com/Level/transcoder#encoding-interface), [`level-codec`](https://github.com/Level/codec#encoding-format), [`abstract-encoding`](https://github.com/mafintosh/abstract-encoding), [`multiformats`](https://github.com/multiformats/js-multiformats/blob/master/src/codecs/interface.ts)
- A previously normalized encoding, such that `keyEncoding(x)` equals `keyEncoding(keyEncoding(x))`
- Omitted, `null` or `undefined`, in which case the default `keyEncoding` of the database is returned.

Other methods that take `keyEncoding` or `valueEncoding` options, accept the same as above. Results are cached. If the `encoding` argument is an object and it has a name then subsequent calls can refer to that encoding by name.

Depending on the encodings supported by a database, this method may return a _transcoder encoding_ that translates the desired encoding from / to an encoding supported by the database. Its `encode()` and `decode()` methods will have respectively the same input and output types as a non-transcoded encoding, but its `name` property will differ.

Assume that e.g. `db.keyEncoding().encode(key)` is safe to call at any time including if the database isn't open, because encodings must be stateless. If the given encoding is not found or supported, a [`LEVEL_ENCODING_NOT_FOUND` or `LEVEL_ENCODING_NOT_SUPPORTED` error](#errors) is thrown.

### `encoding = db.valueEncoding([encoding])`

Same as `db.keyEncoding([encoding])` except that it returns the default `valueEncoding` of the database (if the `encoding` argument is omitted, `null` or `undefined`).

### `key = db.prefixKey(key, keyFormat)`

Add sublevel prefix to the given `key`, which must be already-encoded. If this database is not a sublevel, the given `key` is returned as-is. The `keyFormat` must be one of `'utf8'`, `'buffer'`, `'view'`. If `'utf8'` then `key` must be a string and the return value will be a string. If `'buffer'` then Buffer, if `'view'` then Uint8Array.

```js
const sublevel = db.sublevel('example')

console.log(db.prefixKey('a', 'utf8')) // 'a'
console.log(sublevel.prefixKey('a', 'utf8')) // '!example!a'
```

### `db.defer(fn)`

Call the function `fn` at a later time when [`db.status`](#dbstatus) changes to `'open'` or `'closed'`. Used by `abstract-level` itself to implement "deferred open" which is a feature that makes it possible to call operations like `db.put()` before the database has finished opening. The `defer()` method is exposed for implementations and plugins to achieve the same on their custom operations:

```js
db.foo = function (key, callback) {
  if (this.status === 'opening') {
    this.defer(() => this.foo(key, callback))
  } else {
    // ..
  }
}
```

When deferring a custom operation, do it early: after normalizing optional arguments but before encoding (to avoid double encoding and to emit original input if the operation has events) and before any _fast paths_ (to avoid calling back before the database has finished opening). For example, `db.batch([])` has an internal fast path where it skips work if the array of operations is empty. Resources that can be closed on their own (like iterators) should however first check such state before deferring, in order to reject operations after close (including when the database was reopened).

### `chainedBatch`

#### `chainedBatch.put(key, value[, options])`

Queue a `put` operation on this batch, not committed until `write()` is called. This will throw a [`LEVEL_INVALID_KEY`](#errors) or [`LEVEL_INVALID_VALUE`](#errors) error if `key` or `value` is invalid. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `valueEncoding`: custom value encoding for this operation, used to encode the `value`.
- `sublevel` (sublevel instance): act as though the `put` operation is performed on the given sublevel, to similar effect as `sublevel.batch().put(key, value)`. This allows atomically committing data to multiple sublevels. The `key` will be prefixed with the `prefix` of the sublevel, and the `key` and `value` will be encoded by the sublevel (using the default encodings of the sublevel unless `keyEncoding` and / or `valueEncoding` are provided).

#### `chainedBatch.del(key[, options])`

Queue a `del` operation on this batch, not committed until `write()` is called. This will throw a [`LEVEL_INVALID_KEY`](#errors) error if `key` is invalid. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `sublevel` (sublevel instance): act as though the `del` operation is performed on the given sublevel, to similar effect as `sublevel.batch().del(key)`. This allows atomically committing data to multiple sublevels. The `key` will be prefixed with the `prefix` of the sublevel, and the `key` will be encoded by the sublevel (using the default key encoding of the sublevel unless `keyEncoding` is provided).

#### `chainedBatch.clear()`

Clear all queued operations on this batch.

#### `chainedBatch.write([options][, callback])`

Commit the queued operations for this batch. All operations will be written atomically, that is, they will either all succeed or fail with no partial commits.

There are no `options` by default but implementations may add theirs. Note that `write()` does not take encoding options. Those can only be set on `put()` and `del()` because implementations may synchronously forward such calls to an underlying store and thus need keys and values to be encoded at that point.

The `callback` function will be called with no arguments if the batch was successful or with an error if it failed. If no callback is provided, a promise is returned.

After `write()` or `close()` has been called, no further operations are allowed.

#### `chainedBatch.close([callback])`

Free up underlying resources. This should be done even if the chained batch has zero queued operations. Automatically called by `write()` so normally not necessary to call, unless the intent is to discard a chained batch without committing it. The `callback` function will be called with no arguments. If no callback is provided, a promise is returned. Closing the batch is an idempotent operation, such that calling `close()` more than once is allowed and makes no difference.

#### `chainedBatch.length`

The number of queued operations on the current batch.

#### `chainedBatch.db`

A reference to the database that created this chained batch.

### `iterator`

An iterator allows one to lazily read a range of entries stored in the database. The entries will be sorted by keys in [lexicographic order](https://en.wikipedia.org/wiki/Lexicographic_order) (in other words: byte order) which in short means key `'a'` comes before `'b'` and key `'10'` comes before `'2'`.

An iterator reads from a snapshot of the database, created at the time `db.iterator()` was called. This means the iterator will not see the data of simultaneous write operations. Most but not all implementations can offer this guarantee, as indicated by `db.supports.snapshots`.

Iterators can be consumed with [`for await...of`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of) and `iterator.all()`, or by manually calling `iterator.next()` or `nextv()` in succession. In the latter case, `iterator.close()` must always be called. In contrast, finishing, throwing, breaking or returning from a `for await...of` loop automatically calls `iterator.close()`, as does `iterator.all()`.

An iterator reaches its natural end in the following situations:

- The end of the database has been reached
- The end of the range has been reached
- The last `iterator.seek()` was out of range.

An iterator keeps track of calls that are in progress. It doesn't allow concurrent `next()`, `nextv()` or `all()` calls (including a combination thereof) and will throw an error with code [`LEVEL_ITERATOR_BUSY`](#errors) if that happens:

```js
// Not awaited and no callback provided
iterator.next()

try {
  // Which means next() is still in progress here
  iterator.all()
} catch (err) {
  console.log(err.code) // 'LEVEL_ITERATOR_BUSY'
}
```

#### `for await...of iterator`

Yields entries, which are arrays containing a `key` and `value`. The type of `key` and `value` depends on the options passed to `db.iterator()`.

```js
try {
  for await (const [key, value] of db.iterator()) {
    console.log(key)
  }
} catch (err) {
  console.error(err)
}
```

Note for implementors: this uses `iterator.next()` and `iterator.close()` under the hood so no further method implementations are needed to support `for await...of`.

#### `iterator.next([callback])`

Advance to the next entry and yield that entry. If an error occurs, the `callback` function will be called with an error. Otherwise, the `callback` receives `null`, a `key` and a `value`. The type of `key` and `value` depends on the options passed to `db.iterator()`. If the iterator has reached its natural end, both `key` and `value` will be `undefined`.

If no callback is provided, a promise is returned for either an entry array (containing a `key` and `value`) or `undefined` if the iterator reached its natural end.

**Note:** `iterator.close()` must always be called once there's no intention to call `next()` or `nextv()` again. Even if such calls yielded an error and even if the iterator reached its natural end. Not closing the iterator will result in memory leaks and may also affect performance of other operations if many iterators are unclosed and each is holding a snapshot of the database.

#### `iterator.nextv(size[, options][, callback])`

Advance repeatedly and get at most `size` amount of entries in a single call. Can be faster than repeated `next()` calls. The `size` argument must be an integer and has a soft minimum of 1. There are no `options` by default but implementations may add theirs.

If an error occurs, the `callback` function will be called with an error. Otherwise, the `callback` receives `null` and an array of entries, where each entry is an array containing a key and value. The natural end of the iterator will be signaled by yielding an empty array. If no callback is provided, a promise is returned.

```js
const iterator = db.iterator()

while (true) {
  const entries = await iterator.nextv(100)

  if (entries.length === 0) {
    break
  }

  for (const [key, value] of entries) {
    // ..
  }
}

await iterator.close()
```

#### `iterator.all([options][, callback])`

Advance repeatedly and get all (remaining) entries as an array, automatically closing the iterator. Assumes that those entries fit in memory. If that's not the case, instead use `next()`, `nextv()` or `for await...of`. There are no `options` by default but implementations may add theirs. If an error occurs, the `callback` function will be called with an error. Otherwise, the `callback` receives `null` and an array of entries, where each entry is an array containing a key and value. If no callback is provided, a promise is returned.

```js
const entries = await db.iterator({ limit: 100 }).all()

for (const [key, value] of entries) {
  // ..
}
```

#### `iterator.seek(target[, options])`

Seek to the key closest to `target`. Subsequent calls to `iterator.next()`, `nextv()` or `all()` (including implicit calls in a `for await...of` loop) will yield entries with keys equal to or larger than `target`, or equal to or smaller than `target` if the `reverse` option passed to `db.iterator()` was true.

The optional `options` object may contain:

- `keyEncoding`: custom key encoding, used to encode the `target`. By default the `keyEncoding` option of the iterator is used or (if that wasn't set) the `keyEncoding` of the database.

If range options like `gt` were passed to `db.iterator()` and `target` does not fall within that range, the iterator will reach its natural end.

**Note:** Not all implementations support `seek()`. Consult `db.supports.seek` or the [support matrix](https://github.com/Level/supports#seek-boolean).

#### `iterator.close([callback])`

Free up underlying resources. The `callback` function will be called with no arguments. If no callback is provided, a promise is returned. Closing the iterator is an idempotent operation, such that calling `close()` more than once is allowed and makes no difference.

If a `next()` ,`nextv()` or `all()` call is in progress, closing will wait for that to finish. After `close()` has been called, further calls to `next()` ,`nextv()` or `all()` will yield an error with code [`LEVEL_ITERATOR_NOT_OPEN`](#errors).

#### `iterator.db`

A reference to the database that created this iterator.

#### `iterator.count`

Read-only getter that indicates how many keys have been yielded so far (by any method) excluding calls that errored or yielded `undefined`.

#### `iterator.limit`

Read-only getter that reflects the `limit` that was set in options. Greater than or equal to zero. Equals `Infinity` if no limit, which allows for easy math:

```js
const hasMore = iterator.count < iterator.limit
const remaining = iterator.limit - iterator.count
```

### `keyIterator`

A key iterator has the same interface as `iterator` except that its methods yield keys instead of entries. For the `keyIterator.next(callback)` method, this means that the `callback` will receive two arguments (an error and key) instead of three. Usage is otherwise the same.

### `valueIterator`

A value iterator has the same interface as `iterator` except that its methods yield values instead of entries. For the `valueIterator.next(callback)` method, this means that the `callback` will receive two arguments (an error and value) instead of three. Usage is otherwise the same.

### `sublevel`

A sublevel is an instance of the `AbstractSublevel` class, which extends `AbstractLevel` and thus has the same API as documented above. Sublevels have a few additional properties.

#### `sublevel.prefix`

Prefix of the sublevel. A read-only string property.

```js
const example = db.sublevel('example')
const nested = example.sublevel('nested')

console.log(example.prefix) // '!example!'
console.log(nested.prefix) // '!example!!nested!'
```

#### `sublevel.db`

Parent database. A read-only property.

```js
const example = db.sublevel('example')
const nested = example.sublevel('nested')

console.log(example.db === db) // true
console.log(nested.db === db) // true
```

### Encodings

Any method that takes a `key` argument, `value` argument or range options like `gte`, hereby jointly referred to as `data`, runs that `data` through an _encoding_. This means to encode input `data` and decode output `data`.

[Several encodings](https://github.com/Level/transcoder#built-in-encodings) are builtin courtesy of [`level-transcoder`](https://github.com/Level/transcoder) and can be selected by a short name like `'utf8'` or `'json'`. The default encoding is `'utf8'` which ensures you'll always get back a string. Encodings can be specified for keys and values independently with `keyEncoding` and `valueEncoding` options, either in the database constructor or per method to apply an encoding selectively. For example:

```js
const db = level('./db', {
  keyEncoding: 'view',
  valueEncoding: 'json'
})

// Use binary keys
const key = Uint8Array.from([1, 2])

// Encode the value with JSON
await db.put(key, { x: 2 })

// Decode the value with JSON. Yields { x: 2 }
const obj = await db.get(key)

// Decode the value with utf8. Yields '{"x":2}'
const str = await db.get(key, { valueEncoding: 'utf8' })
```

The `keyEncoding` and `valueEncoding` options accept a string to select a known encoding by its name, or an object to use a custom encoding like [`charwise`](https://github.com/dominictarr/charwise). See [`keyEncoding()`](#encoding--dbkeyencodingencoding) for details. If a custom encoding is passed to the database constructor, subsequent method calls can refer to that encoding by name. Supported encodings are exposed in the `db.supports` manifest:

```js
const db = level('./db', {
  keyEncoding: require('charwise'),
  valueEncoding: 'json'
})

// Includes builtin and custom encodings
console.log(db.supports.encodings.utf8) // true
console.log(db.supports.encodings.charwise) // true
```

An encoding can both widen and limit the range of `data` types. The default `'utf8'` encoding can only store strings. Other types, though accepted, are irreversibly stringified before storage. That includes JavaScript primitives which are converted with [`String(x)`](https://tc39.es/ecma262/multipage/text-processing.html#sec-string-constructor-string-value), Buffer which is converted with [`x.toString('utf8')`](https://nodejs.org/api/buffer.html#buftostringencoding-start-end) and Uint8Array converted with [`TextDecoder#decode(x)`](https://developer.mozilla.org/en-US/docs/Web/API/TextDecoder/decode). Use other encodings for a richer set of `data` types, as well as binary data without a conversion cost - or loss of non-unicode bytes.

For binary data two builtin encodings are available: `'buffer'` and `'view'`. They use a Buffer or Uint8Array respectively. To some extent these encodings are interchangeable, as the `'buffer'` encoding also accepts Uint8Array as input `data` (and will convert that to a Buffer without copying the underlying ArrayBuffer), the `'view'` encoding also accepts Buffer as input `data` and so forth. Output `data` will be either a Buffer or Uint8Array respectively and can also be converted:

```js
const db = level('./db', { valueEncoding: 'view' })
const buffer = await db.get('example', { valueEncoding: 'buffer' })
```

In browser environments it may be preferable to only use `'view'`. When bundling JavaScript with Webpack, Browserify or other, you can choose not to use the `'buffer'` encoding and (through configuration of the bundler) exclude the [`buffer`](https://github.com/feross/buffer) shim in order to reduce bundle size.

Regardless of the choice of encoding, a `key` or `value` may not be `null` or `undefined` due to preexisting significance in iterators and streams. No such restriction exists on range options because `null` and `undefined` are significant types in encodings like [`charwise`](https://github.com/dominictarr/charwise) as well as some underlying stores like IndexedDB. Consumers of an `abstract-level` implementation must assume that range options like `{ gt: undefined }` are _not_ the same as `{}`. The [abstract test suite](#test-suite) does not test these types. Whether they are supported or how they sort may differ per implementation. An implementation can choose to:

- Encode these types to make them meaningful
- Have no defined behavior (moving the concern to a higher level)
- Delegate to an underlying database (moving the concern to a lower level).

Lastly, one way or another, every implementation _must_ support `data` of type String and _should_ support `data` of type Buffer or Uint8Array.

### Events

An `abstract-level` database is an [`EventEmitter`](https://nodejs.org/api/events.html) and emits the following events.

| Event     | Description          | Arguments            |
| :-------- | :------------------- | :------------------- |
| `put`     | Entry was updated    | `key, value` (any)   |
| `del`     | Entry was deleted    | `key` (any)          |
| `batch`   | Batch has executed   | `operations` (array) |
| `clear`   | Entries were deleted | `options` (object)   |
| `opening` | Database is opening  | -                    |
| `open`    | Database has opened  | -                    |
| `ready`   | Alias for `open`     | -                    |
| `closing` | Database is closing  | -                    |
| `closed`  | Database has closed. | -                    |

For example you can do:

```js
db.on('put', function (key, value) {
  console.log('Updated', { key, value })
})
```

Any keys, values and range options in these events are the original arguments passed to the relevant operation that triggered the event, before having encoded them.

### Errors

Errors thrown or yielded from the methods above will have a `code` property that is an uppercase string. Error codes will not change between major versions, but error messages will. Messages may also differ between implementations; they are free and encouraged to tune messages.

A database may also throw [`TypeError`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/TypeError) errors (or other core error constructors in JavaScript) without a `code` and without any guarantee on the stability of error properties - because these errors indicate invalid arguments and other programming mistakes that should not be catched much less have associated logic.

Error codes will be one of the following.

#### `LEVEL_NOT_FOUND`

When a key was not found.

#### `LEVEL_DATABASE_NOT_OPEN`

When an operation was made on a database while it was closing or closed. Or when a database failed to `open()` including when `close()` was called in the mean time, thus changing the eventual `status`. The error may have a `cause` property that explains a failure to open:

```js
try {
  await db.open()
} catch (err) {
  console.error(err.code) // 'LEVEL_DATABASE_NOT_OPEN'

  if (err.cause && err.cause.code === 'LEVEL_LOCKED') {
    // Another process or instance has opened the database
  }
}
```

#### `LEVEL_DATABASE_NOT_CLOSED`

When a database failed to `close()`. Including when `open()` was called in the mean time, thus changing the eventual `status`. The error may have a `cause` property that explains a failure to close.

#### `LEVEL_ITERATOR_NOT_OPEN`

When an operation was made on an iterator while it was closing or closed, which may also be the result of the database being closed.

#### `LEVEL_ITERATOR_BUSY`

When `iterator.next()` or `seek()` was called while a previous `next()` call was still in progress.

#### `LEVEL_BATCH_NOT_OPEN`

When an operation was made on a chained batch while it was closing or closed, which may also be the result of the database being closed or that `write()` was called on the chained batch.

#### `LEVEL_ENCODING_NOT_FOUND`

When a `keyEncoding` or `valueEncoding` option specified a named encoding that does not exist.

#### `LEVEL_ENCODING_NOT_SUPPORTED`

When a `keyEncoding` or `valueEncoding` option specified an encoding that isn't supported by the database.

#### `LEVEL_DECODE_ERROR`

When decoding of keys or values failed. The error _may_ have a [`cause`](https://github.com/tc39/proposal-error-cause) property containing an original error. For example, it might be a [`SyntaxError`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SyntaxError) from an internal [`JSON.parse()`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/JSON/parse) call:

```js
await db.put('key', 'invalid json', { valueEncoding: 'utf8' })

try {
  const value = await db.get('key', { valueEncoding: 'json' })
} catch (err) {
  console.log(err.code) // 'LEVEL_DECODE_ERROR'
  console.log(err.cause) // 'SyntaxError: Unexpected token i in JSON at position 0'
}
```

#### `LEVEL_INVALID_KEY`

When a key is `null`, `undefined` or (if an implementation deems it so) otherwise invalid.

#### `LEVEL_INVALID_VALUE`

When a value is `null`, `undefined` or (if an implementation deems it so) otherwise invalid.

#### `LEVEL_CORRUPTION`

Data could not be read (from an underlying store) due to a corruption.

#### `LEVEL_IO_ERROR`

Data could not be read (from an underlying store) due to an input/output error, for example from the filesystem.

#### `LEVEL_INVALID_PREFIX`

When a sublevel prefix contains characters outside of the supported byte range.

#### `LEVEL_NOT_SUPPORTED`

When a module needs a certain feature, typically as indicated by `db.supports`, but that feature is not available on a database argument or other. For example, some kind of plugin may depend on `seek()`:

```js
const ModuleError = require('module-error')

module.exports = function plugin (db) {
  if (!db.supports.seek) {
    throw new ModuleError('Database must support seeking', {
      code: 'LEVEL_NOT_SUPPORTED'
    })
  }

  // ..
}
```

#### `LEVEL_LEGACY`

When a method, option or other property was used that has been removed from the API.

#### `LEVEL_LOCKED`

When an attempt was made to open a database that is already open in another process or instance. Used by `classic-level` and other implementations of `abstract-level` that use exclusive locks.

#### `LEVEL_READONLY`

When an attempt was made to write data to a read-only database. Used by `many-level`.

#### `LEVEL_CONNECTION_LOST`

When a database relies on a connection to a remote party and that connection has been lost. Used by `many-level`.

#### `LEVEL_REMOTE_ERROR`

When a remote party encountered an unexpected condition that it can't reflect with a more specific code. Used by `many-level`.

### Shared Access

Unless documented otherwise, implementations of `abstract-level` do _not_ support accessing a database from multiple processes running in parallel. That includes Node.js clusters and Electron renderer processes.

See [`Level/awesome`](https://github.com/Level/awesome#shared-access) for modules like [`multileveldown`](https://github.com/Level/multileveldown) and [`level-party`](https://github.com/Level/party) that allow a database to be shared across processes and/or machines. Note: at the time of writing these modules are not yet ported to `abstract-level` and thus incompatible.

## Private API For Implementors

To implement an `abstract-level` database, extend the [`AbstractLevel`](./abstract-level.js) class and override the private underscored versions of its methods. For example, to implement the public `put()` method, override the private `_put()` method. The same goes for other classes (some of which are optional to override). All classes can be found on the main export of the npm package:

```js
const {
  AbstractLevel,
  AbstractSublevel,
  AbstractIterator,
  AbstractKeyIterator,
  AbstractValueIterator,
  AbstractChainedBatch
} = require('abstract-level')
```

Naming-wise, implementations should use a class name in the form of `*Level` (suffixed, for example `MemoryLevel`) and an npm package name in the form of `*-level` (for example `memory-level`). While utilities and plugins should use a package name in the form of `level-*` (prefixed).

Each of the private methods listed below will receive exactly the number and types of arguments described, regardless of what is passed in through the public API. Public methods provide type checking: if a consumer calls `db.batch(123)` they'll get an error that the first argument must be an array. Optional arguments get sensible defaults: a `db.get(key)` call translates to a `db._get(key, options, callback)` call.

All callbacks are error-first and must be asynchronous. If an operation within your implementation is synchronous, invoke the callback on a next tick using microtask scheduling. For convenience, instances of `AbstractLevel`, `AbstractIterator` and other classes include a `nextTick(fn, ...args)` utility method that uses [`process.nextTick()`](https://nodejs.org/api/process.html#processnexttickcallback-args) in Node.js and [`queueMicrotask()`](https://developer.mozilla.org/en-US/docs/Web/API/queueMicrotask) in browsers. It's recommended to exclusively use this utility (including in unit tests) as it guarantees a consistent order of operations.

Where possible, the default private methods are sensible noops that do nothing. For example, `db._open(callback)` will merely invoke `callback` on a next tick. Other methods have functional defaults. Each method documents whether implementing it is mandatory.

When throwing or yielding an error, prefer using a [known error code](#errors). If new codes are required for your implementation and you wish to use the `LEVEL_` prefix for consistency, feel free to open an issue to discuss. We'll likely want to document those codes here.

### Example

Let's implement a basic in-memory database:

```js
const { AbstractLevel } = require('abstract-level')
const ModuleError = require('module-error')

class ExampleLevel extends AbstractLevel {
  // This in-memory example doesn't have a location argument
  constructor (options) {
    // Declare supported encodings
    const encodings = { utf8: true }

    // Call AbstractLevel constructor
    super({ encodings }, options)

    // Create a map to store entries
    this._entries = new Map()
  }

  _open (options, callback) {
    // Here you would open any necessary resources.
    // Use nextTick to be a nice async citizen
    this.nextTick(callback)
  }

  _put (key, value, options, callback) {
    this._entries.set(key, value)
    this.nextTick(callback)
  }

  _get (key, options, callback) {
    const value = this._entries.get(key)

    if (value === undefined) {
      return this.nextTick(callback, new ModuleError(`Key ${key} was not found`, {
        code: 'LEVEL_NOT_FOUND'
      }))
    }

    this.nextTick(callback, null, value)
  }

  _del (key, options, callback) {
    this._entries.delete(key)
    this.nextTick(callback)
  }
}
```

Now we can use our implementation (with either callbacks or promises):

```js
const db = new ExampleLevel()

await db.put('foo', 'bar')
const value = await db.get('foo')

console.log(value) // 'bar'
```

Although our basic implementation only supports `'utf8'` strings internally, we do get to use [encodings](#encodings) that encode _to_ that. For example, the `'json'` encoding which encodes to `'utf8'`:

```js
const db = new ExampleLevel({ valueEncoding: 'json' })
await db.put('foo', { a: 123 })
const value = await db.get('foo')

console.log(value) // { a: 123 }
```

See [`memory-level`](https://github.com/Level/memory-level) if you are looking for a complete in-memory implementation. The example above notably lacks iterator support and would not pass the [abstract test suite](#test-suite).

### `db = AbstractLevel(manifest[, options])`

The database constructor. Sets the [`status`](#dbstatus) to `'opening'`. Takes a [manifest](https://github.com/Level/supports) object that the constructor will enrich with defaults. At minimum, the manifest must declare which `encodings` are supported in the private API. For example:

```js
class ExampleLevel extends AbstractLevel {
  constructor (location, options) {
    const manifest = {
      encodings: { buffer: true }
    }

    // Call AbstractLevel constructor.
    // Location is not handled by AbstractLevel.
    super(manifest, options)
  }
}
```

Both the public and private API of `abstract-level` are encoding-aware. This means that private methods receive `keyEncoding` and `valueEncoding` options too. Implementations don't need to perform encoding or decoding themselves. Rather, the `keyEncoding` and `valueEncoding` options are lower-level encodings that indicate the type of already-encoded input data or the expected type of yet-to-be-decoded output data. They're one of `'buffer'`, `'view'`, `'utf8'` and always strings in the private API.

If the manifest declared support of `'buffer'`, then `keyEncoding` and `valueEncoding` will always be `'buffer'`. If the manifest declared support of `'utf8'` then `keyEncoding` and `valueEncoding` will be `'utf8'`.

For example: a call like `await db.put(key, { x: 2 }, { valueEncoding: 'json' })` will encode the `{ x: 2 }` value and might forward it to the private API as `db._put(key, '{"x":2}', { valueEncoding: 'utf8' }, callback)`. Same for the key (omitted for brevity).

The public API will coerce user input as necessary. If the manifest declared support of `'utf8'` then `await db.get(24)` will forward that number key as a string: `db._get('24', { keyEncoding: 'utf8', ... }, callback)`. However, this is _not_ true for output: a private API call like `db._get(key, { keyEncoding: 'utf8', valueEncoding: 'utf8' }, callback)` _must_ yield a string value to the callback.

All private methods below that take a `key` argument, `value` argument or range option, will receive that data in encoded form. That includes `iterator._seek()` with its `target` argument. So if the manifest declared support of `'buffer'` then `db.iterator({ gt: 2 })` translates into `db._iterator({ gt: Buffer.from('2'), ...options })` and `iterator.seek(128)` translates into `iterator._seek(Buffer.from('128'), options)`.

The `AbstractLevel` constructor will add other supported encodings to the public manifest. If the private API only supports `'buffer'`, the resulting `db.supports.encodings` will nevertheless be as follows because all other encodings can be transcoded to `'buffer'`:

```js
{ buffer: true, view: true, utf8: true, json: true, ... }
```

Implementations can also declare support of multiple encodings. Keys and values will then be encoded and decoded via the most optimal path. For example, in [`classic-level`](https://github.com/Level/classic-level) (previously `leveldown`) it's:

```js
super({ encodings: { buffer: true, utf8: true } }, options, callback)
```

This has the benefit that user input needs less conversion steps: if the input is a string then `classic-level` can pass that to its LevelDB binding as-is. Vice versa for output.

### `db._open(options, callback)`

Open the database. The `options` object will always have the following properties: `createIfMissing`, `errorIfExists`. If opening failed, call the `callback` function with an error. Otherwise call `callback` without any arguments.

The default `_open()` is a sensible noop and invokes `callback` on a next tick.

### `db._close(callback)`

Close the database. When this is called, `db.status` will be `'closing'`. If closing failed, call the `callback` function with an error, which resets the `status` to `'open'`. Otherwise call `callback` without any arguments, which sets `status` to `'closed'`. Make an effort to avoid failing, or if it does happen that it is subsequently safe to keep using the database. If the database was never opened or failed to open then `_close()` will not be called.

The default `_close()` is a sensible noop and invokes `callback` on a next tick. In native implementations (native addons written in C++ or other) it's recommended to delay closing if any operations are in flight. See [`classic-level`](https://github.com/Level/classic-level) (previously `leveldown`) for an example of this behavior. The JavaScript side in `abstract-level` will prevent _new_ operations before the database is reopened (as explained in constructor documentation above) while the C++ side should prevent closing the database before _existing_ operations have completed.

### `db._get(key, options, callback)`

Get a value by `key`. The `options` object will always have the following properties: `keyEncoding` and `valueEncoding`. If the key does not exist, call the `callback` function with an error that has code [`LEVEL_NOT_FOUND`](#errors). Otherwise call `callback` with `null` as the first argument and the value as the second.

The default `_get()` invokes `callback` on a next tick with a `LEVEL_NOT_FOUND` error. It must be overridden.

### `db._getMany(keys, options, callback)`

Get multiple values by an array of `keys`. The `options` object will always have the following properties: `keyEncoding` and `valueEncoding`. If an error occurs, call the `callback` function with an error. Otherwise call `callback` with `null` as the first argument and an array of values as the second. If a key does not exist, set the relevant value to `undefined`.

The default `_getMany()` invokes `callback` on a next tick with an array of values that is equal in length to `keys` and is filled with `undefined`. It must be overridden.

### `db._put(key, value, options, callback)`

Add a new entry or overwrite an existing entry. The `options` object will always have the following properties: `keyEncoding` and `valueEncoding`. If putting failed, call the `callback` function with an error. Otherwise call `callback` without any arguments.

The default `_put()` invokes `callback` on a next tick. It must be overridden.

### `db._del(key, options, callback)`

Delete an entry. The `options` object will always have the following properties: `keyEncoding`. If deletion failed, call the `callback` function with an error. Otherwise call `callback` without any arguments.

The default `_del()` invokes `callback` on a next tick. It must be overridden.

### `db._batch(operations, options, callback)`

Perform multiple _put_ and/or _del_ operations in bulk. The `operations` argument is always an array containing a list of operations to be executed sequentially, although as a whole they should be performed as an atomic operation. The `_batch()` method will not be called if the `operations` array is empty. Each operation is guaranteed to have at least `type`, `key` and `keyEncoding` properties. If the type is `put`, the operation will also have `value` and `valueEncoding` properties. There are no default options but `options` will always be an object. If the batch failed, call the `callback` function with an error. Otherwise call `callback` without any arguments.

The public `batch()` method supports encoding options both in the `options` argument and per operation. The private `_batch()` method only receives encoding options per operation.

The default `_batch()` invokes `callback` on a next tick. It must be overridden.

### `db._chainedBatch()`

The default `_chainedBatch()` returns a functional `AbstractChainedBatch` instance that uses `db._batch(array, options, callback)` under the hood. To implement chained batch in an optimized manner, extend `AbstractChainedBatch` and return an instance of this class in the `_chainedBatch()` method:

```js
const { AbstractChainedBatch } = require('abstract-level')

class ExampleChainedBatch extends AbstractChainedBatch {
  constructor (db) {
    super(db)
  }
}

class ExampleLevel extends AbstractLevel {
  _chainedBatch () {
    return new ExampleChainedBatch(this)
  }
}
```

### `db._iterator(options)`

The default `_iterator()` returns a noop `AbstractIterator` instance. It must be overridden, by extending `AbstractIterator` and returning an instance of this class in the `_iterator(options)` method:

```js
const { AbstractIterator } = require('abstract-level')

class ExampleIterator extends AbstractIterator {
  constructor (db, options) {
    super(db, options)
  }

  // ..
}

class ExampleLevel extends AbstractLevel {
  _iterator (options) {
    return new ExampleIterator(this, options)
  }
}
```

The `options` object will always have the following properties: `reverse`, `keys`, `values`, `limit`, `keyEncoding` and `valueEncoding`. The `limit` will always be an integer, greater than or equal to `-1` and less than `Infinity`. If the user passed range options to `db.iterator()`, those will be encoded and set in `options`.

### `db._keys(options)`

The default `_keys()` returns a functional iterator that wraps `db._iterator()` in order to map entries to keys. For optimal performance it can be overridden by extending `AbstractKeyIterator`:

```js
const { AbstractKeyIterator } = require('abstract-level')

class ExampleKeyIterator extends AbstractKeyIterator {
  constructor (db, options) {
    super(db, options)
  }

  // ..
}

class ExampleLevel extends AbstractLevel {
  _keys (options) {
    return new ExampleKeyIterator(this, options)
  }
}
```

The `options` object will always have the following properties: `reverse`, `limit` and `keyEncoding`. The `limit` will always be an integer, greater than or equal to `-1` and less than `Infinity`. If the user passed range options to `db.keys()`, those will be encoded and set in `options`.

### `db._values(options)`

The default `_values()` returns a functional iterator that wraps `db._iterator()` in order to map entries to values. For optimal performance it can be overridden by extending `AbstractValueIterator`:

```js
const { AbstractValueIterator } = require('abstract-level')

class ExampleValueIterator extends AbstractValueIterator {
  constructor (db, options) {
    super(db, options)
  }

  // ..
}

class ExampleLevel extends AbstractLevel {
  _values (options) {
    return new ExampleValueIterator(this, options)
  }
}
```

The `options` object will always have the following properties: `reverse`, `limit`, `keyEncoding` and `valueEncoding`. The `limit` will always be an integer, greater than or equal to -1 and less than Infinity. If the user passed range options to `db.values()`, those will be encoded and set in `options`.

### `db._clear(options, callback)`

Delete all entries or a range. Does not have to be atomic. It is recommended (and possibly mandatory in the future) to operate on a snapshot so that writes scheduled after a call to `clear()` will not be affected.

Implementations that wrap another database can typically forward the `_clear()` call to that database, having transformed range options if necessary.

The `options` object will always have the following properties: `reverse`, `limit` and `keyEncoding`. If the user passed range options to `db.clear()`, those will be encoded and set in `options`.

### `sublevel = db._sublevel(name, options)`

Create a [sublevel](#sublevel). The `options` object will always have the following properties: `separator`. The default `_sublevel()` returns a new instance of the [`AbstractSublevel`](./lib/abstract-sublevel.js) class. Overriding is optional. The `AbstractSublevel` can be extended in order to add additional methods to sublevels:

```js
const { AbstractLevel, AbstractSublevel } = require('abstract-level')

class ExampleLevel extends AbstractLevel {
  _sublevel (name, options) {
    return new ExampleSublevel(this, name, options)
  }
}

// For brevity this does not handle deferred open
class ExampleSublevel extends AbstractSublevel {
  example (key, options) {
    // Encode and prefix the key
    const keyEncoding = this.keyEncoding(options.keyEncoding)
    const keyFormat = keyEncoding.format

    key = this.prefixKey(keyEncoding.encode(key), keyFormat)

    // The parent database can be accessed like so. Make sure
    // to forward encoding options and use the full key.
    this.db.del(key, { keyEncoding: keyFormat }, ...)
  }
}
```

### `iterator = AbstractIterator(db, options)`

The first argument to this constructor must be an instance of the relevant `AbstractLevel` implementation. The constructor will set `iterator.db` which is used (among other things) to access encodings and ensures that `db` will not be garbage collected in case there are no other references to it. The `options` argument must be the original `options` object that was passed to `db._iterator()` and it is therefore not (publicly) possible to create an iterator via constructors alone.

#### `iterator._next(callback)`

Advance to the next entry and yield that entry. If the operation failed, call the `callback` function with an error. Otherwise, call `callback` with `null`, a `key` and a `value`. If a `limit` was set and the iterator already yielded that many entries (via any of the methods) then `_next()` will not be called.

The default `_next()` invokes `callback` on a next tick. It must be overridden.

#### `iterator._nextv(size, options, callback)`

Advance repeatedly and get at most `size` amount of entries in a single call. The `size` argument will always be an integer greater than 0. If a `limit` was set then `size` will be at most `limit - iterator.count`. If a `limit` was set and the iterator already yielded that many entries (via any of the methods) then `_nextv()` will not be called. There are no default options but `options` will always be an object. If the operation failed, call the `callback` function with an error. Otherwise, call `callback` with `null` and an array of entries. An empty array signifies the natural end of the iterator, so yield an array with at least one entry if the end has not been reached yet.

The default `_nextv()` is a functional default that makes repeated calls to `_next()` and should be overridden for better performance.

#### `iterator._all(options, callback)`

Advance repeatedly and get all (remaining) entries as an array. If a `limit` was set and the iterator already yielded that many entries (via any of the methods) then `_all()` will not be called. Do not call `close()` here because `all()` will do so (regardless of any error) and this may become an opt-out behavior in the future. There are no default options but `options` will always be an object. If the operation failed, call the `callback` function with an error. Otherwise, call `callback` with `null` and an array of entries.

The default `_all()` is a functional default that makes repeated calls to `_nextv()` and should be overridden for better performance.

#### `iterator._seek(target, options)`

Seek to the key closest to `target`. The `options` object will always have the following properties: `keyEncoding`. This method is optional. The default will throw an error with code [`LEVEL_NOT_SUPPORTED`](#errors). If supported, set `db.supports.seek` to `true` (via the manifest passed to the database constructor) which also enables relevant tests in the [test suite](#test-suite).

#### `iterator._close(callback)`

Free up underlying resources. This method is guaranteed to only be called once. Once closing is done, call `callback` without any arguments. It is not allowed to yield an error.

The default `_close()` invokes `callback` on a next tick. Overriding is optional.

### `keyIterator = AbstractKeyIterator(db, options)`

A key iterator has the same interface and constructor arguments as `AbstractIterator` except that it must yields keys instead of entries. For the `keyIterator._next(callback)` method, this means that the `callback` must be given two arguments (an error and key) instead of three. The same goes for value iterators:

```js
class ExampleKeyIterator extends AbstractKeyIterator {
  _next (callback) {
    this.nextTick(callback, null, 'key')
  }
}

class ExampleValueIterator extends AbstractValueIterator {
  _next (callback) {
    this.nextTick(callback, null, 'value')
  }
}
```

The `options` argument must be the original `options` object that was passed to `db._keys()` and it is therefore not (publicly) possible to create a key iterator via constructors alone. The same goes for value iterators via `db._values()`.

**Note:** the `AbstractKeyIterator` and `AbstractValueIterator` classes do _not_ extend the `AbstractIterator` class. Similarly, if your implementation overrides `db._keys()` returning a custom subclass of `AbstractKeyIterator`, then that subclass must implement methods like `_next()` separately from your subclass of `AbstractIterator`.

### `valueIterator = AbstractValueIterator(db, options)`

A value iterator has the same interface and constructor arguments as `AbstractIterator` except that it must yields values instead of entries. For further details, see `keyIterator` above.

### `chainedBatch = AbstractChainedBatch(db)`

The first argument to this constructor must be an instance of the relevant `AbstractLevel` implementation. The constructor will set `chainedBatch.db` which is used (among other things) to access encodings and ensures that `db` will not be garbage collected in case there are no other references to it.

#### `chainedBatch._put(key, value, options)`

Queue a `put` operation on this batch. The `options` object will always have the following properties: `keyEncoding` and `valueEncoding`.

#### `chainedBatch._del(key, options)`

Queue a `del` operation on this batch. The `options` object will always have the following properties: `keyEncoding`.

#### `chainedBatch._clear()`

Clear all queued operations on this batch.

#### `chainedBatch._write(options, callback)`

The default `_write` method uses `db._batch`. If the `_write` method is overridden it must atomically commit the queued operations. There are no default options but `options` will always be an object. If committing fails, call the `callback` function with an error. Otherwise call `callback` without any arguments. The `_write()` method will not be called if the chained batch has zero queued operations.

#### `chainedBatch._close(callback)`

Free up underlying resources. This method is guaranteed to only be called once. Once closing is done, call `callback` without any arguments. It is not allowed to yield an error.

The default `_close()` invokes `callback` on a next tick. Overriding is optional.

## Test Suite

To prove that your implementation is `abstract-level` compliant, include the abstract test suite in your `test.js` (or similar):

```js
const test = require('tape')
const suite = require('abstract-level/test')
const ExampleLevel = require('.')

suite({
  test,
  factory (options) {
    return new ExampleLevel(options)
  }
})
```

The `test` option _must_ be a function that is API-compatible with [`tape`](https://github.com/substack/tape). The `factory` option _must_ be a function that returns a unique and isolated instance of your implementation. The factory will be called many times by the test suite.

If your implementation is disk-based we recommend using [`tempy`](https://github.com/sindresorhus/tempy) (or similar) to create unique temporary directories. Your setup could look something like:

```js
const test = require('tape')
const tempy = require('tempy')
const suite = require('abstract-level/test')
const ExampleLevel = require('.')

suite({
  test,
  factory (options) {
    return new ExampleLevel(tempy.directory(), options)
  }
})
```

### Excluding tests

As not every implementation can be fully compliant due to limitations of its underlying storage, some tests may be skipped. This must be done via `db.supports` which is set via the constructor. For example, to skip snapshot tests:

```js
const { AbstractLevel } = require('abstract-level')

class ExampleLevel extends AbstractLevel {
  constructor (location, options) {
    super({ snapshots: false }, options)
  }
}
```

This also serves as a signal to users of your implementation.

### Reusing `testCommon`

The input to the test suite is a `testCommon` object. Should you need to reuse `testCommon` for your own (additional) tests, use the included utility to create a `testCommon` with defaults:

```js
const test = require('tape')
const suite = require('abstract-level/test')
const ExampleLevel = require('.')

const testCommon = suite.common({
  test,
  factory (options) {
    return new ExampleLevel(options)
  }
})

suite(testCommon)
```

The `testCommon` object will have the `test` and `factory` properties described above, as well as a convenience `supports` property that is lazily copied from a `factory().supports`. You might use it like so:

```js
test('custom test', function (t) {
  const db = testCommon.factory()
  // ..
})

testCommon.supports.seek && test('another test', function (t) {
  const db = testCommon.factory()
  // ..
})
```

## Spread The Word

If you'd like to share your awesome implementation with the world, here's what you might want to do:

- Add an awesome badge to your `README`: `![level badge](https://leveljs.org/img/badge.svg)`
- Publish your awesome module to [npm](https://npmjs.org)
- Send a Pull Request to [Level/awesome](https://github.com/Level/awesome) to advertise your work!

## Install

With [npm](https://npmjs.org) do:

```
npm install abstract-level
```

## Contributing

[`Level/abstract-level`](https://github.com/Level/abstract-level) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

Support us with a monthly donation on [Open Collective](https://opencollective.com/level) and help us continue our work.

## License

[MIT](LICENSE)

[level-badge]: https://leveljs.org/img/badge.svg
