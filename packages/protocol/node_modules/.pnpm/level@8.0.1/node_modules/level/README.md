# level

**Universal [`abstract-level`](https://github.com/Level/abstract-level) database for Node.js and browsers.** This is a convenience package that exports [`classic-level`](https://github.com/Level/classic-level) in Node.js and [`browser-level`](https://github.com/Level/browser-level) in browsers, making it an ideal entry point to start creating lexicographically sorted key-value databases.

> :pushpin: Which module should I use? What is `abstract-level`? Head over to the [FAQ](https://github.com/Level/community#faq).

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/level.svg)](https://www.npmjs.com/package/level)
[![Node version](https://img.shields.io/node/v/level.svg)](https://www.npmjs.com/package/level)
[![Test](https://img.shields.io/github/actions/workflow/status/Level/level/test.yml?branch=master&label=test)](https://github.com/Level/level/actions/workflows/test.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/level?label=\&logo=codecov\&logoColor=fff)](https://codecov.io/gh/Level/level)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript\&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Community](https://img.shields.io/badge/community-join-%2370B99E?logo=github)](https://github.com/Level/community/issues)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective\&logoColor=fff)](https://opencollective.com/level)

## Table of Contents

<details><summary>Click to expand</summary>

- [Usage](#usage)
- [Install](#install)
- [Supported Platforms](#supported-platforms)
- [API](#api)
  - [`db = new Level(location[, options])`](#db--new-levellocation-options)
  - [`db.status`](#dbstatus)
  - [`db.open([callback])`](#dbopencallback)
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
- [Contributing](#contributing)
- [Donate](#donate)
- [License](#license)

</details>

## Usage

_If you are upgrading: please see [`UPGRADING.md`](UPGRADING.md)._

```js
const { Level } = require('level')

// Create a database
const db = new Level('example', { valueEncoding: 'json' })

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

TypeScript type declarations are included and cover the methods that are common between `classic-level` and `browser-level`. Usage from TypeScript requires generic type parameters.

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

## Install

With [npm](https://npmjs.org) do:

```bash
npm install level
```

For use in browsers, this package is best used with [`browserify`](https://github.com/browserify/browserify), [`webpack`](https://webpack.js.org/), [`rollup`](https://rollupjs.org/) or similar bundlers. For a quick start, visit [`browserify-starter`](https://github.com/Level/browserify-starter) or [`webpack-starter`](https://github.com/Level/webpack-starter).

## Supported Platforms

At the time of writing, `level` works in Node.js 12+ and Electron 5+ on Linux, Mac OS, Windows and FreeBSD, including any future Node.js and Electron release thanks to [Node-API](https://nodejs.org/api/n-api.html), including ARM platforms like Raspberry Pi and Android, as well as in Chrome, Firefox, Edge, Safari, iOS Safari and Chrome for Android. For details, see [Supported Platforms](https://github.com/Level/classic-level#supported-platforms) of `classic-level` and [Browser Support](https://github.com/Level/browser-level#browser-support) of `browser-level`.

Binary keys and values are supported across the board.

## API

The API of `level` follows that of [`abstract-level`](https://github.com/Level/abstract-level). The documentation below covers it all except for [Encodings](https://github.com/Level/abstract-level#encodings), [Events](https://github.com/Level/abstract-level#events) and [Errors](https://github.com/Level/abstract-level#errors) which are exclusively documented in `abstract-level`. For options and additional methods specific to [`classic-level`](https://github.com/Level/classic-level) and [`browser-level`](https://github.com/Level/browser-level), please see their respective READMEs.

An `abstract-level` and thus `level` database is at its core a [key-value database](https://en.wikipedia.org/wiki/Key%E2%80%93value_database). A key-value pair is referred to as an _entry_ here and typically returned as an array, comparable to [`Object.entries()`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Object/entries).

### `db = new Level(location[, options])`

Create a new database or open an existing database. The `location` argument must be a directory path (relative or absolute) where LevelDB will store its files, or in browsers, the name of the [`IDBDatabase`](https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase) to be opened.

The optional `options` object may contain:

- `keyEncoding` (string or object, default `'utf8'`): encoding to use for keys
- `valueEncoding` (string or object, default `'utf8'`): encoding to use for values.

See [Encodings](https://github.com/Level/abstract-level#encodings) for a full description of these options. Other `options` (except `passive`) are forwarded to `db.open()` which is automatically called in a next tick after the constructor returns. Any read & write operations are queued internally until the database has finished opening. If opening fails, those queued operations will yield errors.

### `db.status`

Read-only getter that returns a string reflecting the current state of the database:

- `'opening'` - waiting for the database to be opened
- `'open'` - successfully opened the database
- `'closing'` - waiting for the database to be closed
- `'closed'` - successfully closed the database.

### `db.open([callback])`

Open the database. The `callback` function will be called with no arguments when successfully opened, or with a single error argument if opening failed. If no callback is provided, a promise is returned. Options passed to `open()` take precedence over options passed to the database constructor. The `createIfMissing` and `errorIfExists` options are not supported by [`browser-level`](https://github.com/Level/browser-level).

The optional `options` object may contain:

- `createIfMissing` (boolean, default: `true`): If `true`, create an empty database if one doesn't already exist. If `false` and the database doesn't exist, opening will fail.
- `errorIfExists` (boolean, default: `false`): If `true` and the database already exists, opening will fail.
- `passive` (boolean, default: `false`): Wait for, but do not initiate, opening of the database.

It's generally not necessary to call `open()` because it's automatically called by the database constructor. It may however be useful to capture an error from failure to open, that would otherwise not surface until another method like `db.get()` is called. It's also possible to reopen the database after it has been closed with [`close()`](#dbclosecallback). Once `open()` has then been called, any read & write operations will again be queued internally until opening has finished.

The `open()` and `close()` methods are idempotent. If the database is already open, the `callback` will be called in a next tick. If opening is already in progress, the `callback` will be called when that has finished. If closing is in progress, the database will be reopened once closing has finished. Likewise, if `close()` is called after `open()`, the database will be closed once opening has finished and the prior `open()` call will receive an error.

### `db.close([callback])`

Close the database. The `callback` function will be called with no arguments if closing succeeded or with a single `error` argument if closing failed. If no callback is provided, a promise is returned.

A database may have associated resources like file handles and locks. When the database is no longer needed (for the remainder of a program) it's recommended to call `db.close()` to free up resources.

After `db.close()` has been called, no further read & write operations are allowed unless and until `db.open()` is called again. For example, `db.get(key)` will yield an error with code [`LEVEL_DATABASE_NOT_OPEN`](https://github.com/Level/abstract-level#errors). Any unclosed iterators or chained batches will be closed by `db.close()` and can then no longer be used even when `db.open()` is called again.

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

The `callback` function will be called with an error if the operation failed. If the key was not found, the error will have code [`LEVEL_NOT_FOUND`](https://github.com/Level/abstract-level#errors). If successful the first argument will be `null` and the second argument will be the value. If no callback is provided, a promise is returned.

### `db.getMany(keys[, options][, callback])`

Get multiple values from the database by an array of `keys`. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `keys`.
- `valueEncoding`: custom value encoding for this operation, used to decode values.

The `callback` function will be called with an error if the operation failed. If successful the first argument will be `null` and the second argument will be an array of values with the same order as `keys`. If a key was not found, the relevant value will be `undefined`. If no callback is provided, a promise is returned.

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

Create a [chained batch](#chainedbatch), when `batch()` is called with zero arguments. A chained batch can be used to build and eventually commit an atomic batch of operations. Depending on how it's used, it is possible to obtain greater performance with this form of `batch()`. On `browser-level` however, it is just sugar.

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

Create a [sublevel](#sublevel) that has the same interface as `db` (except for additional methods specific to `classic-level` or `browser-level`) and prefixes the keys of operations before passing them on to `db`. The `name` argument is required and must be a string.

```js
const example = db.sublevel('example')

await example.put('hello', 'world')
await db.put('a', '1')

// Prints ['hello', 'world']
for await (const [key, value] of example.iterator()) {
  console.log([key, value])
}
```

Sublevels effectively separate a database into sections. Think SQL tables, but evented, ranged and real-time! Each sublevel is an `AbstractLevel` instance with its own keyspace, [events](https://github.com/Level/abstract-level#events) and [encodings](https://github.com/Level/abstract-level#encodings). For example, it's possible to have one sublevel with `'buffer'` keys and another with `'utf8'` keys. The same goes for values. Like so:

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

The `keyEncoding` and `valueEncoding` options are forwarded to the `AbstractLevel` constructor and work the same, as if a new, separate database was created. They default to `'utf8'` regardless of the encodings configured on `db`. Other options are forwarded too but `abstract-level` (and therefor `level`) has no relevant options at the time of writing. For example, setting the `createIfMissing` option will have no effect. Why is that?

Like regular databases, sublevels open themselves but they do not affect the state of the parent database. This means a sublevel can be individually closed and (re)opened. If the sublevel is created while the parent database is opening, it will wait for that to finish. If the parent database is closed, then opening the sublevel will fail and subsequent operations on the sublevel will yield errors with code [`LEVEL_DATABASE_NOT_OPEN`](https://github.com/Level/abstract-level#errors).

### `chainedBatch`

#### `chainedBatch.put(key, value[, options])`

Queue a `put` operation on this batch, not committed until `write()` is called. This will throw a [`LEVEL_INVALID_KEY`](https://github.com/Level/abstract-level#errors) or [`LEVEL_INVALID_VALUE`](https://github.com/Level/abstract-level#errors) error if `key` or `value` is invalid. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `valueEncoding`: custom value encoding for this operation, used to encode the `value`.
- `sublevel` (sublevel instance): act as though the `put` operation is performed on the given sublevel, to similar effect as `sublevel.batch().put(key, value)`. This allows atomically committing data to multiple sublevels. The `key` will be prefixed with the `prefix` of the sublevel, and the `key` and `value` will be encoded by the sublevel (using the default encodings of the sublevel unless `keyEncoding` and / or `valueEncoding` are provided).

#### `chainedBatch.del(key[, options])`

Queue a `del` operation on this batch, not committed until `write()` is called. This will throw a [`LEVEL_INVALID_KEY`](https://github.com/Level/abstract-level#errors) error if `key` is invalid. The optional `options` object may contain:

- `keyEncoding`: custom key encoding for this operation, used to encode the `key`.
- `sublevel` (sublevel instance): act as though the `del` operation is performed on the given sublevel, to similar effect as `sublevel.batch().del(key)`. This allows atomically committing data to multiple sublevels. The `key` will be prefixed with the `prefix` of the sublevel, and the `key` will be encoded by the sublevel (using the default key encoding of the sublevel unless `keyEncoding` is provided).

#### `chainedBatch.clear()`

Clear all queued operations on this batch.

#### `chainedBatch.write([options][, callback])`

Commit the queued operations for this batch. All operations will be written atomically, that is, they will either all succeed or fail with no partial commits.

There are no `options` (that are common between `classic-level` and `browser-level`). Note that `write()` does not take encoding options. Those can only be set on `put()` and `del()`.

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

A `classic-level` iterator reads from a snapshot of the database, created at the time `db.iterator()` was called. This means the iterator will not see the data of simultaneous write operations. A `browser-level` iterator does not offer such guarantees, as is indicated by `db.supports.snapshots`. That property will be true in Node.js and false in browsers.

Iterators can be consumed with [`for await...of`](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Statements/for-await...of) and `iterator.all()`, or by manually calling `iterator.next()` or `nextv()` in succession. In the latter case, `iterator.close()` must always be called. In contrast, finishing, throwing, breaking or returning from a `for await...of` loop automatically calls `iterator.close()`, as does `iterator.all()`.

An iterator reaches its natural end in the following situations:

- The end of the database has been reached
- The end of the range has been reached
- The last `iterator.seek()` was out of range.

An iterator keeps track of calls that are in progress. It doesn't allow concurrent `next()`, `nextv()` or `all()` calls (including a combination thereof) and will throw an error with code [`LEVEL_ITERATOR_BUSY`](https://github.com/Level/abstract-level#errors) if that happens:

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

#### `iterator.next([callback])`

Advance to the next entry and yield that entry. If an error occurs, the `callback` function will be called with an error. Otherwise, the `callback` receives `null`, a `key` and a `value`. The type of `key` and `value` depends on the options passed to `db.iterator()`. If the iterator has reached its natural end, both `key` and `value` will be `undefined`.

If no callback is provided, a promise is returned for either an entry array (containing a `key` and `value`) or `undefined` if the iterator reached its natural end.

**Note:** `iterator.close()` must always be called once there's no intention to call `next()` or `nextv()` again. Even if such calls yielded an error and even if the iterator reached its natural end. Not closing the iterator will result in memory leaks and may also affect performance of other operations if many iterators are unclosed and each is holding a snapshot of the database.

#### `iterator.nextv(size[, options][, callback])`

Advance repeatedly and get at most `size` amount of entries in a single call. Can be faster than repeated `next()` calls. The `size` argument must be an integer and has a soft minimum of 1. There are no `options` at the moment.

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

Advance repeatedly and get all (remaining) entries as an array, automatically closing the iterator. Assumes that those entries fit in memory. If that's not the case, instead use `next()`, `nextv()` or `for await...of`. There are no `options` at the moment. If an error occurs, the `callback` function will be called with an error. Otherwise, the `callback` receives `null` and an array of entries, where each entry is an array containing a key and value. If no callback is provided, a promise is returned.

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

#### `iterator.close([callback])`

Free up underlying resources. The `callback` function will be called with no arguments. If no callback is provided, a promise is returned. Closing the iterator is an idempotent operation, such that calling `close()` more than once is allowed and makes no difference.

If a `next()` ,`nextv()` or `all()` call is in progress, closing will wait for that to finish. After `close()` has been called, further calls to `next()` ,`nextv()` or `all()` will yield an error with code [`LEVEL_ITERATOR_NOT_OPEN`](https://github.com/Level/abstract-level#errors).

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

## Contributing

[`Level/level`](https://github.com/Level/level) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

Support us with a monthly donation on [Open Collective](https://opencollective.com/level) and help us continue our work.

## License

[MIT](LICENSE)

[level-badge]: https://leveljs.org/img/badge.svg
