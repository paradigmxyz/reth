# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## Table of Contents

<details><summary>Click to expand</summary>

- [1.0.0](#100)
  - [1. API parity with `levelup`](#1-api-parity-with-levelup)
    - [1.1. New: promises](#11-new-promises)
    - [1.2. New: events](#12-new-events)
    - [1.3. New: idempotent open](#13-new-idempotent-open)
    - [1.4. New: deferred open](#14-new-deferred-open)
    - [1.5. No constructor callback](#15-no-constructor-callback)
    - [1.6. New: state checks](#16-new-state-checks)
    - [1.7. New: chained batch length](#17-new-chained-batch-length)
  - [2. API parity with `level`](#2-api-parity-with-level)
    - [2.1. For consumers](#21-for-consumers)
    - [2.2. For implementors](#22-for-implementors)
    - [2.3. Other notable changes](#23-other-notable-changes)
  - [3. Streams have moved](#3-streams-have-moved)
  - [4. Zero-length keys and range options are now valid](#4-zero-length-keys-and-range-options-are-now-valid)
  - [5. Resources are auto-closed](#5-resources-are-auto-closed)
    - [5.1. Closing iterators is idempotent](#51-closing-iterators-is-idempotent)
    - [5.2. Chained batch can be closed](#52-chained-batch-can-be-closed)
  - [6. Errors now use codes](#6-errors-now-use-codes)
  - [7. Semi-private properties have been removed](#7-semi-private-properties-have-been-removed)
  - [8. Changes to test suite](#8-changes-to-test-suite)
  - [9. Sublevels are builtin](#9-sublevels-are-builtin)

</details>

## 1.0.0

**Introducing `abstract-level`: a fork of [`abstract-leveldown`](https://github.com/Level/abstract-leveldown) that removes the need for [`levelup`](https://github.com/Level/levelup), [`encoding-down`](https://github.com/Level/encoding-down) and more. An `abstract-level` database is a complete solution that doesn't need to be wrapped. It has the same API as `level(up)` including encodings, promises and events. In addition, implementations can now choose to use Uint8Array instead of Buffer. Consumers of an implementation can use both. Sublevels are builtin.**

We've put together several upgrade guides for different modules. See the [FAQ](https://github.com/Level/community#faq) to find the best upgrade guide for you. This upgrade guide describes how to replace `abstract-leveldown` with `abstract-level`. Implementations that do so, can no longer be wrapped with `levelup`.

The npm package name is `abstract-level` and the main export is called `AbstractLevel` rather than `AbstractLevelDOWN`. It started using classes. Support of Node.js 10 has been dropped.

For most folks, a database that upgraded from `abstract-leveldown` to `abstract-level` can be a drop-in replacement for a `level(up)` database (with the exception of stream methods). Let's start this guide there: all methods have been enhanced to reach API parity with `levelup` and `level`.

### 1. API parity with `levelup`

#### 1.1. New: promises

Methods that take a callback now also support promises. They return a promise if no callback is provided, the same as `levelup`. Implementations that override public (non-underscored) methods _must_ do the same and any implementation _should_ do the same for additional methods if any.

#### 1.2. New: events

An `abstract-level` database emits the same events as `levelup` would.

#### 1.3. New: idempotent open

Opening and closing a database is idempotent and safe, similar to `levelup` but more precise. If `open()` and `close()` are called repeatedly, the last call dictates the final status. Callbacks are not called (or promises not resolved) until any pending state changes are done. Same for events. Unlike on `levelup` it is safe to call `open()` while status is `'closing'`: the database will wait for closing to complete and then reopen. None of these changes are likely to constitute a breaking change; they increase state consistency in edge cases.

The `open()` method has a new option called `passive`. If set to `true` the call will wait for, but not initiate, opening of the database. To similar effect as `db.once('open', callback)` with added benefit that it also works if the database is already open. Implementations that wrap another database can use the `passive` option to open themselves without taking full control of the database that they wrap.

#### 1.4. New: deferred open

Deferred open is built-in. This means a database opens itself a tick after its constructor returns (unless `open()` was called manually). Any operations made until opening has completed are queued up in memory. When opening completes the operations are replayed. If opening failed (and this is a new behavior compared to `levelup`) the operations will yield errors. The `AbstractLevel` class has a new `defer()` method for an implementation to defer custom operations.

The initial `status` of a database is `'opening'` rather than `'new'`, which no longer exists. Wrapping a database with [`deferred-leveldown`](https://github.com/Level/deferred-leveldown) is not supported and will exhibit undefined behavior.

Implementations must also accept options for `open()` in their constructor, which was previously done by `levelup`. For example, usage of the [`classic-level`](https://github.com/Level/classic-level) implementation is as follows:

```js
const db = new ClassicLevel('./db', {
  createIfMissing: false,
  compression: false
})
```

This works by first forwarding options to the `AbstractLevel` constructor, which in turn forwards them to `open(options)`. If `open(options)` is called manually those options will be shallowly merged with options from the constructor:

```js
// Results in { createIfMissing: false, compression: true }
await db.open({ compression: true })
```

A database is not "patch-safe". If some form of plugin monkey-patches a database like in the following example, it must now also take the responsibility of deferring the operation (as well as handling promises and callbacks) using `db.defer()`. I.e. this example is incomplete:

```js
function plugin (db) {
  const original = db.get

  db.get = function (...args) {
    original.call(this, ...args)
  }
}
```

#### 1.5. No constructor callback

The database constructor does not take a callback argument, unlike `levelup`. This goes for `abstract-level` as well as implementations - which is to say, implementors don't have to (and should not) support this old pattern.

Instead call `db.open()` if you wish to wait for opening (which is not necessary to use the database) or to capture an error. If that's your reason for using the callback and you previously initialized a database like so (simplified):

```js
levelup(function (err, db) {
  // ..
})
```

You must now do:

```js
db.open(function (err) {
  // ..
})
```

Or using promises:

```js
await db.open()
```

#### 1.6. New: state checks

On any operation, an `abstract-level` database checks if it's open. If not, it will either throw an error (if the relevant API is synchronous) or asynchronously yield an error. For example:

```js
await db.close()

try {
  db.iterator()
} catch (err) {
  console.log(err.code) // LEVEL_DATABASE_NOT_OPEN
}
```

_Errors now have a `code` property. More on that below\._

This may be a breaking change downstream because it changes error messages for implementations that had their own safety checks (which will now be ineffective because `abstract-level` checks are performed first) or implicitly relied on `levelup` checks. By safety we mean mainly that yielding a JavaScript error is preferred over segmentation faults, though non-native implementations also benefit from detecting incorrect usage.

Implementations that have additional methods should add or align their own safety checks for consistency. Like so:

<details>
<summary>Click to expand</summary>

```js
const ModuleError = require('module-error')

class ExampleLevel extends AbstractLevel {
  // For brevity this example does not implement promises or encodings
  approximateSize (start, end, callback) {
    if (this.status === 'opening') {
      this.defer(() => this.approximateSize(start, end, callback))
    } else if (this.status !== 'open') {
      this.nextTick(callback, new ModuleError('Database is not open', {
        code: 'LEVEL_DATABASE_NOT_OPEN'
      }))
    } else {
      // ..
    }
  }
}
```

</details>

#### 1.7. New: chained batch length

The `AbstractChainedBatch` prototype has a new `length` property that, like a chained batch in `levelup`, returns the number of queued operations in the batch. Implementations should not have to make changes for this unless they monkey-patched public methods of `AbstractChainedBatch`.

### 2. API parity with `level`

It was previously necessary to use [`level`](https://github.com/Level/level) to get the "full experience". Or similar modules like [`level-mem`](https://github.com/Level/mem), [`level-rocksdb`](https://github.com/Level/level-rocksdb) and more. These modules combined an `abstract-leveldown` implementation with [`encoding-down`](https://github.com/Level/encoding-down) and [`levelup`](https://github.com/Level/levelup). Encodings are now built-in to `abstract-level`, using [`level-transcoder`](https://github.com/Level/transcoder) rather than [`level-codec`](https://github.com/Level/codec). The main change is that logic from the existing public API has been expanded down into the storage layer.

The `level` module still has a place, for its support of both Node.js and browsers and for being the main entrypoint into the Level ecosystem. The next major version of `level`, that's v8.0.0, will likely simply export [`classic-level`](https://github.com/Level/classic-level) in Node.js and [`browser-level`](https://github.com/Level/browser-level) in browsers. To differentiate, the text below will refer to the old version as `level@7`.

#### 2.1. For consumers

All relevant methods including the database constructor now accept `keyEncoding` and `valueEncoding` options, the same as `level@7`. Read operations now yield strings rather than buffers by default, having the same default `'utf8'` encoding as `level@7` and friends.

There are a few differences from `level@7` and `encoding-down`. Some breaking:

- The lesser-used `'ascii'`, `'ucs2'` and `'utf16le'` encodings are not supported
- The `'id'` encoding, which was not supported by any active `abstract-leveldown` implementation and aliased as `'none'`, has been removed
- The undocumented `encoding` option (as an alias for `valueEncoding`) is not supported.

And some non-breaking:

- The `'binary'` encoding has been renamed to `'buffer'`, with `'binary'` as an alias
- The `'utf8'` encoding previously did not touch Buffers. Now it will call `buffer.toString('utf8')` for consistency. Consumers can use the `'buffer'` encoding to avoid this conversion.

If you previously did one of the following (on a database that's defaulting to the `'utf8'` encoding):

```js
await db.put('a', Buffer.from('x'))
await db.put('a', Buffer.from('x'), { valueEncoding: 'binary' })
```

Both examples will still work (assuming the buffer contains only UTF8 data) but you should now do:

```js
await db.put('a', Buffer.from('x'), { valueEncoding: 'buffer' })
```

Or use the new `'view'` encoding which accepts Uint8Arrays (and therefore also Buffer):

```js
await db.put('a', new Uint8Array(...), { valueEncoding: 'view' })
```

#### 2.2. For implementors

_You can skip this section if you're consuming (rather than writing) an `abstract-level` implementation._

Both the public and private API of `abstract-level` are encoding-aware. This means that private methods receive `keyEncoding` and `valueEncoding` options too, instead of the `keyAsBuffer`, `valueAsBuffer` and `asBuffer` options that `abstract-leveldown` had. Implementations don't need to perform encoding or decoding themselves. In fact they can do less: the `_serializeKey()` and `_serializeValue()` methods are also gone and implementations are less likely to have to convert between strings and buffers.

For example: a call like `db.put(key, { x: 2 }, { valueEncoding: 'json' })` will encode the `{ x: 2 }` value and might forward it to the private API as `db._put(key, '{"x":2}', { valueEncoding: 'utf8' }, callback)`. Same for the key, omitted for brevity. We say "might" because it depends on the implementation, which can now declare which encodings it supports.

To first give a concrete example for `get()`, if your implementation previously did:

```js
class ExampleLeveldown extends AbstractLevelDOWN {
  _get (key, options, callback) {
    if (options.asBuffer) {
      this.nextTick(callback, null, Buffer.from('abc'))
    } else {
      this.nextTick(callback, null, 'abc')
    }
  }
}
```

You must now do (if still relevant):

```js
class ExampleLevel extends AbstractLevel {
  _get (key, options, callback) {
    if (options.valueEncoding === 'buffer') {
      this.nextTick(callback, null, Buffer.from('abc'))
    } else {
      this.nextTick(callback, null, 'abc')
    }
  }
}
```

The encoding options and data received by the private API depend on which encodings it supports. It must declare those via the manifest passed to the `AbstractLevel` constructor. See the [`README`](README.md) for details. For example, an implementation might only support storing data as Uint8Arrays, known here as the `'view'` encoding:

```js
class ExampleLevel extends AbstractLevel {
  constructor (location, options) {
    super({ encodings: { view: true } }, options)
  }
}
```

The earlier `put()` example would then result in `db._put(key, value, { valueEncoding: 'view' }, callback)` where `value` is a Uint8Array containing JSON in binary form. And the earlier `_get()` example can be simplified to:

```js
class ExampleLevel extends AbstractLevel {
  _get (key, options, callback) {
    // No need to check valueEncoding as it's always 'view'
    this.nextTick(callback, null, new Uint8Array(...))
  }
}
```

Implementations can also declare support of multiple encodings; keys and values will then be encoded via the most optimal path. For example:

```js
super({
  encodings: {
    view: true,
    utf8: true
  }
})
```

#### 2.3. Other notable changes

- The `AbstractIterator` constructor now requires an `options` argument, for encoding options
- The `AbstractIterator#_seek()` method got a new `options` argument, for a `keyEncoding` option
- The `db.supports.bufferKeys` property has been removed. Use `db.supports.encodings.buffer` instead.

### 3. Streams have moved

Node.js readable streams must now be created with a new standalone module called [`level-read-stream`](https://github.com/Level/read-stream), rather than database methods like `db.createReadStream()`. Please see its [upgrade guide](https://github.com/Level/read-stream/blob/main/UPGRADING.md#100) for details.

To offer an alternative to `db.createKeyStream()` and `db.createValueStream()`, two new types of iterators have been added: `db.keys()` and `db.values()`. Their default implementations are functional but implementors may want to override them for optimal performance. The same goes for two new methods on iterators: `nextv()` and `all()`. To achieve this and honor the `limit` option, abstract iterators now count how many items they yielded, which may remove the need for implementations to do so on their own. Please see the README for details.

### 4. Zero-length keys and range options are now valid

These keys sort before anything else. Historically they weren't supported for causing segmentation faults in `leveldown`. That doesn't apply to today's codebase. Implementations must now support:

```js
await db.put('', 'example')

console.log(await db.get('')) // 'example'

for await (const [key, value] of db.iterator({ lte: '' })) {
  console.log(value) // 'example'
}
```

Same goes for zero-length Buffer and Uint8Array keys. Zero-length keys would previously result in an error and never reach the private API.

### 5. Resources are auto-closed

To further improve safety and consistency, additional changes were made that make an `abstract-level` database safer to use than `abstract-leveldown` wrapped with `levelup`.

#### 5.1. Closing iterators is idempotent

The `iterator.end()` method has been renamed to `iterator.close()`, with `end()` being an alias until a next major version in the future. The term "close" makes it easier to differentiate between the iterator having reached its natural end (data-wise) versus closing it to cleanup resources. If you previously did:

```js
const iterator = db.iterator()
iterator.end(callback)
```

You should now do one of:

```js
iterator.close(callback)
await iterator.close()
```

Likewise, in the private API for implementors, `_end()` has been renamed to `_close()` but without an alias. This method is no longer allowed to yield an error.

On `db.close()`, non-closed iterators are now automatically closed. This may be a breaking change but only if an implementation has (at its own risk) overridden the public `end()` method, because `close()` or `end()` is now an idempotent operation rather than yielding an `end() already called on iterator` error. If a `next()` call is in progress, closing the iterator (or database) will wait for that.

The error message `cannot call next() after end()` has been replaced with code `LEVEL_ITERATOR_NOT_OPEN`, the error `cannot call seek() after end()` has been removed in favor of a silent return, and `cannot call next() before previous next() has completed` and `cannot call seek() before next() has completed` have been replaced with code `LEVEL_ITERATOR_BUSY`.

The `next()` method no longer returns `this` (when a callback is provided).

#### 5.2. Chained batch can be closed

Chained batch has a new method `close()` which is an idempotent operation and automatically called after `write()` (for backwards compatibility) or on `db.close()`. This to ensure batches can't be used after closing and reopening a db. If a `write()` is in progress, closing will wait for that. If `write()` is never called then `close()` must be. For example:

```js
const batch = db.batch()
  .put('abc', 'zyz')
  .del('foo')

if (someCondition) {
  await batch.write()
} else {
  // Decided not to commit
  await batch.close()
}

// In either case this will throw
batch.put('more', 'data')
```

These changes could be breaking for an implementation that has (at its own risk) overridden the public `write()` method. In addition, the error message `write() already called on this batch` has been replaced with code `LEVEL_BATCH_NOT_OPEN`.

An implementation can optionally override `AbstractChainedBatch#_close()` if it has resources to free and wishes to free them earlier than GC would.

### 6. Errors now use codes

The [`level-errors`](https://github.com/Level/errors) module as used by `levelup` and friends, is not used or exposed by `abstract-level`. Instead errors thrown or yielded from a database have a `code` property. See the [`README`](./README.md#errors) for details. Going forward, the semver contract will be on `code` and error messages will change without a semver-major bump.

To minimize breakage, the most used error as yielded by `get()` when an entry is not found, has the same properties that `level-errors` added (`notFound` and `status`) in addition to code `LEVEL_NOT_FOUND`. Those properties will be removed in a future version. Implementations can still yield an error that matches `/NotFound/i.test(err)` or they can start using the code. Either way `abstract-level` will normalize the error.

If you previously did:

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

### 7. Semi-private properties have been removed

The following properties and methods can no longer be accessed, as they've been removed or replaced with internal [symbols](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Symbol):

- `AbstractIterator#_nexting`
- `AbstractIterator#_ended`
- `AbstractChainedBatch#_written`
- `AbstractChainedBatch#_checkWritten()`
- `AbstractChainedBatch#_operations`
- `AbstractLevel#_setupIteratorOptions()`

### 8. Changes to test suite

_You can skip this section if you're consuming (rather than writing) an `abstract-level` implementation._

The abstract test suite of `abstract-level` has some breaking changes compared to `abstract-leveldown`:

- Options to skip tests have been removed in favor of `db.supports`
- Support of `db.clear()` and `db.getMany()` is now mandatory. The default (slow) implementation of `_clear()` has been removed.
- Added tests that `gte` and `lte` range options take precedence over `gt` and `lt` respectively. This is incompatible with [`ltgt`](https://github.com/dominictarr/ltgt) but aligns with `subleveldown`, [`level-option-wrap`](https://github.com/substack/level-option-wrap) and half of `leveldown`. There was no good choice.
- The `setUp` and `tearDown` functions have been removed from the test suite and `suite.common()`.
- Added ability to access manifests via `testCommon.supports`, by lazily copying it from `testCommon.factory().supports`. This requires that the manifest does not change during the lifetime of a `db`.
- Your `factory()` function must now accept an `options` argument.

Many tests were imported from `levelup`, `encoding-down`, `deferred-leveldown`, `memdown`, `level-js` and `leveldown`. They test the changes described above and improve coverage of existing behavior.

Lastly, it's recommended to revisit any custom tests of an implementation. In particular if those tests relied upon the previously loose state checking of `abstract-leveldown`. For example, making a `db.put()` call before `db.open()`. Such a test now has a different meaning. The previous meaning can typically be restored by inserting `db.once('open', ...)` or `await db.open()` logic.

### 9. Sublevels are builtin

_This section is only relevant if you use [`subleveldown`](https://github.com/Level/subleveldown) (which can not wrap an `abstract-level` database)._

Sublevels are now builtin. If you previously did:

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

The key structure is equal to that of `subleveldown`. This means that an `abstract-level` sublevel can read sublevels previously created with (and populated by) `subleveldown`. There are some new features:

- `db.batch(..)` takes a `sublevel` option on operations, to atomically commit data to multiple sublevels
- Sublevels support Uint8Array in addition to Buffer
- `AbstractLevel#_sublevel()` can be overridden to add additional methods to sublevels.

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

Third, the `open` option has been removed. If you need an asynchronous open hook, feel free to open an issue to discuss restoring this API. Should it support promises? Should `abstract-level` support it on any database and not just sublevels?

Lastly, the error message `Parent database is not open` (courtesy of `subleveldown` which had to check open state to prevent segmentation faults from underlying databases) changed to error code [`LEVEL_DATABASE_NOT_OPEN`](https://github.com/Level/abstract-level#errors) (courtesy of `abstract-level` which does those checks on any database).

---

_For earlier releases, before `abstract-level` was forked from `abstract-leveldown` (v7.2.0), please see [the upgrade guide of `abstract-leveldown`](https://github.com/Level/abstract-leveldown/blob/master/UPGRADING.md)._
