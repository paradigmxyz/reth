# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## 8.0.0

**This release replaces `leveldown` and `level-js` with [`classic-level`](https://github.com/Level/classic-level) and [`browser-level`](https://github.com/Level/browser-level). These modules implement the [`abstract-level`](https://github.com/Level/abstract-level) interface instead of [`abstract-leveldown`](https://github.com/Level/abstract-leveldown). This gives them the same API as `level@7` without having to be wrapped with [`levelup`](https://github.com/Level/levelup) or [`encoding-down`](https://github.com/Level/encoding-down). In addition, you can now choose to use Uint8Array instead of Buffer. Sublevels are built-in.**

We've put together several upgrade guides for different modules. See the [FAQ](https://github.com/Level/community#faq) to find the best upgrade guide for you. This one describes how to upgrade `level`.

Support of Node.js 10 has been dropped.

### Changes to initialization

We started using classes, which means using `new` is now required. If you previously did:

```js
const level = require('level')
const db = level('db')
```

You must now do:

```js
const { Level } = require('level')
const db = new Level('db')
```

### TypeScript makes a win

TypeScript type declarations are now included in the npm package(s). For `level` it's an intersection of `classic-level` and `browser-level` types that includes their options but excludes methods like `compactRange()` that can only be found in either. JavaScript folks using VSCode will also benefit from the new types because they enable auto-completion and now include documentation.

### Waking up from limbo

Deferred open - meaning that a database opens itself and any operations made in the mean time are queued up in memory - remains built-in. A new behavior is that those operations will yield errors if opening failed. They'd previously end up in limbo.

An `abstract-level` and thus `level` database is not "patch-safe". If some form of plugin monkey-patches a database method, it must now also take the responsibility of deferring the operation (as well as handling promises and callbacks) using [`db.defer()`](https://github.com/Level/abstract-level#dbdeferfn).

### Creating the location recursively

To align behavior between platforms, `classic-level` and therefore `level@8` creates the location directory recursively. While `leveldown` and therefore `level@7` would only do so on Windows. In the following example, the `foo` directory does not have to exist beforehand:

```js
const db = new Level('foo/bar')
```

This new behavior may break expectations, given typical filesystem behavior, or it could be a convenient feature, if the database is considered to abstract away the filesystem. We're [collecting feedback](https://github.com/Level/classic-level/issues/7) to determine what to do in a next (major) version. Your vote is most welcome!

### No constructor callback

The database constructor no longer takes a callback argument. Instead call `db.open()` if you wish to wait for opening (which is not necessary to use the database) or to capture an error. If that's your reason for using the callback and you previously initialized a database like so:

```js
level('fruits', function (err, db) {
  // ..
})
```

You must now do one of:

```js
db.open(callback)
await db.open()
```

### There is only encodings

Encodings have a new home in `abstract-level` and are now powered by [`level-transcoder`](https://github.com/Level/transcoder). The main change is that logic from the existing public API has been expanded down into the storage layer. There are however a few differences from `level@7`. Some breaking:

- The lesser-used `'id'`, `'ascii'`, `'ucs2'` and `'utf16le'` encodings are not supported
- The undocumented `encoding` option (as an alias for `valueEncoding`) is not supported.

And some non-breaking:

- The `'binary'` encoding has been renamed to `'buffer'`, with `'binary'` as an alias
- The `'utf8'` encoding previously did not touch Buffers. Now it will call `buffer.toString('utf8')` for consistency. Consumers can use the `'buffer'` encoding to avoid this conversion.

Both `classic-level` and `browser-level` support Uint8Array data, in addition to Buffer. It's a separate encoding called `'view'` that can be used interchangeably:

```js
const db = new Level('people', { valueEncoding: 'view' })

await db.put('elena', new Uint8Array([97, 98, 99]))
await db.get('elena') // Uint8Array
await db.get('elena', { valueEncoding: 'utf8' }) // 'abc'
await db.get('elena', { valueEncoding: 'buffer' }) // Buffer
```

For browsers you can choose to use Uint8Array exclusively and omit the [`buffer`](https://github.com/feross/buffer) shim from your JavaScript bundle (through configuration of Webpack, Browserify or other).

### Streams have moved

Node.js readable streams must now be created with a new standalone module called [`level-read-stream`](https://github.com/Level/read-stream) rather than database methods like `db.createReadStream()`. For browsers you might prefer [`level-web-stream`](https://github.com/Level/web-stream) which does not require bundling the [`buffer`](https://github.com/feross/buffer) or [`readable-stream`](https://github.com/nodejs/readable-stream) shims. Both `level-read-stream` and `level-web-stream` can be used in Node.js and browsers. The former is significantly faster (also compared to `level@7`, thanks to a new `nextv()` method on iterators). The latter is a step towards a standard library for JavaScript across Node.js, Deno and browsers.

To offer an alternative to `db.createKeyStream()` and `db.createValueStream()`, two new types of iterators have been added: `db.keys()` and `db.values()`.

### State checks for safety

On any operation, an `abstract-level` and thus `level` database checks if it's open. If not, it will either throw an error (if the relevant API is synchronous) or asynchronously yield an error. For example:

```js
await db.close()

try {
  db.iterator()
} catch (err) {
  console.log(err.code) // LEVEL_DATABASE_NOT_OPEN
}
```

_Errors now have a `code` property. More on that below\._

### Zero-length keys and range options are now valid

These keys sort before anything else. Historically they weren't supported for causing segmentation faults in `leveldown`. That doesn't apply to today's codebase. You can now do:

```js
await db.put('', 'abc')

console.log(await db.get('')) // 'abc'
console.log(await db.get(new Uint8Array(0), { keyEncoding: 'view' })) // 'abc'

for await (const [key, value] of db.iterator({ lte: '' })) {
  console.log(value) // 'abc'
}
```

### It doesn't end there

The `iterator.end()` method has been renamed to `iterator.close()`, with `end()` being an alias until a next major version. The term "close" makes it easier to differentiate between the iterator having reached its natural end (data-wise) versus closing it to cleanup resources. If you previously did:

```js
const iterator = db.iterator()
iterator.end(callback)
```

You should now do one of:

```js
iterator.close(callback)
await iterator.close()
```

On `db.close()`, non-closed iterators are now automatically closed (only for safety reasons). If a call like `next()` is in progress, closing the iterator or database will wait for that. Calling `iterator.close()` more than once is now allowed and makes no difference.

### Other changes to iterators

- In browsers, backpressure is now preferred over snapshot guarantees. For details, please see [`browser-level@1`](https://github.com/Level/browser-level/blob/main/UPGRADING.md#100). On the flip side, `iterator.seek()` now also works in browsers.
- Use of [`level-concat-iterator`](https://github.com/Level/concat-iterator) can be replaced with [`iterator.all()`](https://github.com/Level/level#iteratoralloptions-callback). The former does support `abstract-level` databases but the latter is optimized and always has snapshot guarantees.
- The previously undocumented `highWaterMark` option of `leveldown` is called [`highWaterMarkBytes`](https://github.com/Level/classic-level#about-high-water) in `classic-level` to remove a conflict with streams.
- On iterators with `{ keys: false }` or `{ values: false }` options, the yielded key or value is now consistently `undefined`.

### A chained batch should be closed

Chained batch has a new method `close()` which is an idempotent operation and automatically called after `write()` (for backwards compatibility) or on `db.close()`. This to ensure batches can't be used after closing and reopening a db. If a `write()` is in progress, closing will wait for that. If `write()` is never called then `close()` must be and that's a breaking change because inaction will cause memory leaks. For example:

```js
const batch = db.batch()
  .put('elena', 'abc')
  .del('steve')

if (someCondition) {
  await batch.write()
} else {
  // Decided not to commit
  await batch.close()
}

// In either case this will throw
batch.put('daniel', 'xyz')
```

### Errors now use codes

The [`level-errors`](https://github.com/Level/errors) module is no longer used or exposed by `level@8`. Instead errors thrown or yielded from a database [have a `code` property](https://github.com/Level/abstract-level#errors). Going forward, the semver contract will be on `code` and error messages will change without a semver-major bump.

To minimize breakage, the most used error as yielded by `get()` when an entry is not found, has the same properties that `level-errors` added (`notFound` and `status`) in addition to code `LEVEL_NOT_FOUND`. Those properties will be removed in a future version. If you previously did:

```js
db.get('abc', function (err, value) {
  if (err && err.notFound) {
    // Handle missing entry
  }
})
```

That will still work but it's preferred to do:

```js
db.get('abc', function (err, value) {
  if (err && err.code === 'LEVEL_NOT_FOUND') {
    // Handle missing entry
  }
})
```

Or using promises:

```js
try {
  const value = await db.get('abc')
} catch (err) {
  if (err.code === 'LEVEL_NOT_FOUND') {
    // Handle missing entry
  }
}
```

Side note: it's been suggested more than once to remove this error altogether and we likely will after the dust has settled on `abstract-level`.

### Changes to lesser-used properties and methods

The following properties and methods can no longer be accessed, as they've been removed, renamed or replaced with internal [symbols](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Symbol).

| Object        | Property or method        | Original module      | New module       |
| :------------ | :------------------------ | :------------------- | :--------------- |
| db            | `_setupIteratorOptions()` | `abstract-leveldown` | `abstract-level` |
| db            | `prefix` <sup>1</sup>     | `level-js`           | `browser-level`  |
| db            | `upgrade()`               | `level-js`           | `browser-level`  |
| iterator      | `_nexting`                | `abstract-leveldown` | `abstract-level` |
| iterator      | `_ended`                  | `abstract-leveldown` | `abstract-level` |
| iterator      | `cache` <sup>2</sup>      | `leveldown`          | `classic-level`  |
| iterator      | `finished`                | `leveldown`          | `classic-level`  |
| chained batch | `_written`                | `abstract-leveldown` | `abstract-level` |
| chained batch | `_checkWritten()`         | `abstract-leveldown` | `abstract-level` |
| chained batch | `_operations`             | `abstract-leveldown` | `abstract-level` |

<small>

1. Conflicted with the `db.prefix` property of sublevels. Renamed to `db.namePrefix`.
2. If you were using this then you'll want to checkout the new [`nextv()`](https://github.com/Level/level#iteratornextvsize-options-callback) method.

</small>

The following properties are now read-only getters.

| Object        | Property           | Original module      | New module       |
| :------------ | :----------------- | :------------------- | :--------------- |
| db            | `status`           | `abstract-leveldown` | `abstract-level` |
| db            | `location`         | `leveldown`          | `classic-level`  |
| db            | `location`         | `level-js`           | `browser-level`  |
| db            | `namePrefix`       | `level-js`           | `browser-level`  |
| db            | `version`          | `level-js`           | `browser-level`  |
| db            | `db` (IDBDatabase) | `level-js`           | `browser-level`  |
| chained batch | `length`           | `levelup`            | `abstract-level` |

### Sublevels are built-in

_This section is only relevant if you use [`subleveldown`](https://github.com/Level/subleveldown), which can not wrap a `level@8` database._

If you previously did:

```js
const sub = require('subleveldown')
const example1 = sub(db, 'example1')
const example2 = sub(db, 'example2', { valueEncoding: 'json' })
```

You must now do:

```js
const example1 = db.sublevel('example1')
const example2 = db.sublevel('example2', { valueEncoding: 'json' })
```

The key structure is equal to that of `subleveldown`. This means that a sublevel can read sublevels previously created with (and populated by) `subleveldown`. There are some new features:

- `db.batch(..)` takes a `sublevel` option on operations, to atomically commit data to multiple sublevels
- Sublevels support Uint8Array in addition to Buffer.

To reduce function overloads, the prefix argument (`example1` above) is now required and it's called `name` here. If you previously did one of the following, resulting in an empty name:

```js
subleveldown(db)
subleveldown(db, { separator: '@' })
```

You must now use an explicit empty name:

```js
db.sublevel('')
db.sublevel('', { separator: '@' })
```

The string shorthand for `{ separator }` has also been removed. If you previously did:

```js
subleveldown(db, 'example', '@')
```

You must now do:

```js
db.sublevel('example', { separator: '@' })
```

Third, the `open` option has been removed. If you need an asynchronous open hook, feel free to open an issue to discuss restoring this API.

Lastly, the error message `Parent database is not open` (courtesy of `subleveldown` which had to check open state to prevent segmentation faults from underlying databases) changed to error code [`LEVEL_DATABASE_NOT_OPEN`](https://github.com/Level/abstract-level#errors) (courtesy of `abstract-level` which does those checks on any database).

## 7.0.0

Legacy range options have been removed ([Level/community#86](https://github.com/Level/community/issues/86)). If you previously did:

```js
db.createReadStream({ start: 'a', end: 'z' })
```

An error would now be thrown and you must instead do:

```js
db.createReadStream({ gte: 'a', lte: 'z' })
```

The same applies to `db.iterator()`, `db.createKeyStream()` and `db.createValueStream()`.

This release also drops support of legacy runtime environments ([Level/community#98](https://github.com/Level/community/issues/98)):

- Node.js 6 and 8
- Internet Explorer 11
- Safari 9-11
- Stock Android browser (AOSP).

Lastly, in browsers, the [`immediate`](https://github.com/calvinmetcalf/immediate) and `process` browser shims for `process.nextTick()` have been replaced with the smaller [`queue-microtask`](https://github.com/feross/queue-microtask), except in streams. In the future we might use `queueMicrotask()` in Node.js too.

## 6.0.0

**No breaking changes to the `level` API. If you're only using `level` in Node.js or Electron, you can upgrade without thinking twice.**

The major bump is for browsers, because `level` upgraded to [`level-js@5`](https://github.com/Level/level-js):

> Support of keys & values other than strings and Buffers has been dropped. Internally `level-js` now stores keys & values as binary which solves a number of compatibility issues ([Level/memdown#186](https://github.com/Level/memdown/issues/186)). If you pass in a key or value that isn't a string or Buffer, it will be irreversibly stringified.
>
> Existing IndexedDB databases created with `level-js@4` \[via `level@5`] can be read only if they used binary keys and string or binary values. Other types will come out stringified, and string keys will sort incorrectly. Use the included `upgrade()` utility to convert stored data to binary (in so far the environment supports it):
>
> ```js
> var level = require('level')
> var reachdown = require('reachdown')
> var db = level('my-db')
>
> db.open(function (err) {
>   if (err) throw err
>
>   reachdown(db, 'level-js').upgrade(function (err) {
>     if (err) throw err
>   })
> })
> ```

### New Features :sparkles:

In case you missed it (a few of these already floated into `level@5`) some exciting new features are now available in all environments:

- Added [`db.clear()`](https://github.com/Level/level#dbclearoptions-callback) to delete all entries or a range! Also works in [`subleveldown`](https://github.com/Level/subleveldown) - empty that bucket!
- Check out [`db.supports`](https://github.com/Level/level#supports): a manifest describing the features of a db!
- Glorious: `leveldown` ships a prebuilt binary for Linux that is now [compatible with Debian 8, Ubuntu 14.04, RHEL 7, CentOS 7 and other flavors with an old glibc](https://github.com/Level/leveldown/pull/674)!
- With thanks to [Cirrus CI](https://cirrus-ci.org/), `leveldown` is now [continuously tested in FreeBSD](https://github.com/Level/leveldown/pull/678)!

Go forth and build amazing things.

## 5.0.0

Upgraded to [`leveldown@5.0.0`](https://github.com/Level/leveldown/blob/v5.0.0/UPGRADING.md#v5) and (through `level-packager@5`) [`levelup@4`](https://github.com/Level/levelup/blob/v4.0.0/UPGRADING.md#v4) and [`encoding-down@6`](https://github.com/Level/encoding-down/blob/v6.0.0/UPGRADING.md#v6). Please follow these links for more information. A quick summary: range options (e.g. `gt`) are now serialized the same as keys, `{ gt: undefined }` is not the same as `{}`, nullish values are now rejected and streams are backed by [`readable-stream@3`](https://github.com/nodejs/readable-stream#version-3xx).

In addition, `level` got browser support! It uses [`leveldown`](https://github.com/Level/leveldown) in node and [`level-js`](https://github.com/Level/level-js) in browsers (backed by [IndexedDB](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API)). As such, [`level-browserify`](https://github.com/Level/level-browserify) is not needed anymore and will be deprecated later on. To learn what the integration of `level-js` means for platform, browser and type support, please see the updated [README](README.md#supported-platforms).

## 4.0.0

Dropped support for node 4. No other breaking changes.

## 3.0.0

No breaking changes to the `level` API.

This is an upgrade to `leveldown@^3.0.0` which is based on `abstract-leveldown@~4.0.0` which in turn contains breaking changes to [`.batch()`](https://github.com/Level/abstract-leveldown/commit/a2621ad70571f6ade9d2be42632ece042e068805). Though this is negated by `levelup`, we decided to release a new major version in the event of dependents reaching down into `db.db`.

## 2.0.0

No breaking changes to the `level` API.

The parts that make up `level` have been refactored to increase modularity. This is an upgrade to `leveldown@~2.0.0` and `level-packager@~2.0.0`, which in turn upgraded to `levelup@^2.0.0`. The responsibility of encoding keys and values moved from [`levelup`](https://github.com/Level/levelup) to [`encoding-down`](https://github.com/Level/encoding-down), which comes bundled with [`level-packager`](https://github.com/Level/packager).

Being a convenience package, `level` glues the parts back together to form a drop-in replacement for the users of `levelup@1`, while staying fully compatible with `level@1`. One thing we do get for free, is native Promise support.

```js
const db = level('db')
await db.put('foo', 'bar')
console.log(await db.get('foo'))
```

This does not affect the existing callback API, functionality-wise or performance-wise.

For more information please check the corresponding `CHANGELOG.md` for:

- [`levelup`](https://github.com/Level/levelup/blob/master/CHANGELOG.md)
- [`leveldown`](https://github.com/Level/leveldown/blob/master/CHANGELOG.md)
- [`level-packager`](https://github.com/Level/packager/blob/master/CHANGELOG.md)
