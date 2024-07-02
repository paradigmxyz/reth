# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## 1.0.0

**Introducing `browser-level`: a fork of [`level-js`](https://github.com/Level/level-js) that removes the need for [`levelup`](https://github.com/Level/levelup) and more. It implements the [`abstract-level`](https://github.com/Level/abstract-level) interface instead of [`abstract-leveldown`](https://github.com/Level/abstract-leveldown) and thus has the same API as `level` and `levelup` including encodings, promises and events. In addition, you can now choose to use Uint8Array instead of Buffer. Sublevels are builtin.**

We've put together several upgrade guides for different modules. See the [FAQ](https://github.com/Level/community#faq) to find the best upgrade guide for you. This one describes how to replace `level-js` with `browser-level`. There will be a separate guide for `level`.

### What is not covered here

If you are using any of the following, please also read the upgrade guide of [`abstract-level@1`](https://github.com/Level/abstract-level/blob/main/UPGRADING.md#100) which goes into more detail about these:

- Specific error messages (replaced with error codes)
- The `db.iterator().end()` method (renamed to `close()`, with `end()` as an alias)
- Zero-length keys and range options (now valid)
- The `db.supports.bufferKeys` property.

### Changes to initialization

We started using classes, which means using `new` is now required. If you previously did:

```js
const levelJs = require('level-js')
const db = levelJs('example')
```

You must now do:

```js
const { BrowserLevel } = require('browser-level')
const db = new BrowserLevel('example')
```

Arguments and options are the same except that `browser-level` has additional options to control encodings. For backwards compatibility, the `prefix` option has the same default value. Namely `'level-js-'`.

### There is only encodings

The `asBuffer`, `valueAsBuffer` and `keyAsBuffer` options have been replaced with encoding options. The default encoding is `'utf8'` which means operations return strings rather than Buffers by default. If you previously did:

```js
db.get('example', { asBuffer: false }, callback)
db.get('example', callback)
```

You must now do:

```js
db.get('example', callback)
db.get('example', { valueEncoding: 'buffer' }, callback)
```

Or using promises (new):

```js
const str = await db.get('example')
const buf = await db.get('example', { valueEncoding: 'buffer' })
```

Or using Uint8Array (new):

```js
const arr = await db.get('example', { valueEncoding: 'view' })
```

### Uint8Array support

A `browser-level` database uses Uint8Array internally, instead of Buffer like `level-js` did. This doesn't change the ability to read existing databases: `browser-level` can read and write databases that were created with `level-js` and vice versa. Externally both Uint8Array and Buffer can be used (see README for details) to maintain backwards- and ecosystem compatibility. You can choose to use Uint8Array exclusively and omit the `buffer` shim from a JavaScript bundle (through configuration of Webpack, Browserify or other bundlers).

### Unwrapping the onion

If you were wrapping `level-js` with `levelup`, `encoding-down` and / or `subleveldown`, remove those modules. If you previously did:

```js
const levelJs = require('level-js')
const levelup = require('levelup')
const enc = require('encoding-down')
const subleveldown = require('subleveldown')

const db = levelup(enc(levelJs('example')))
const sublevel = subleveldown(db, 'foo')
```

You must now do:

```js
const { BrowserLevel } = require('browser-level')
const db = new BrowserLevel('example')
const sublevel = db.sublevel('foo')
```

### Prefers backpressure over snapshot guarantees

Previously (in `level-js`) an iterator would keep reading in the background, so as to keep the underlying IndexedDB transaction alive and thus not see the data of simultaneous writes. I.e. it was reading from a snapshot in time. This had two major downsides. It was not possible to lazily iterate a large database, as all of the data would be read into memory. Secondly, IndexedDB doesn't actually use a snapshot (the Chrome implementation used to, others never did) but rather a blocking transaction. Meaning you couldn't write to a database while an iterator was active; the write would wait for the iterator to end.

To solve those issues, an iterator now reads a few entries ahead and then opens a new transaction on the next read. A "few" means all entries for `iterator.all()`, `size` amount of entries for `iterator.nextv(size)` and a hardcoded 100 entries for `iterator.next()`. Individual calls to those methods still have snapshot guarantees, but repeated calls do not.

The resulting breaking change is that an iterator will include the data of simultaneous writes, if `db.put()`, `db.del()` or `db.batch()` are called in between creating the iterator and consuming the iterator, or in between calls to `iterator.next()` or `iterator.nextv()`. For example:

```js
const iterator = db.iterator()
await db.put('abc', '123')

for await (const [key, value] of iterator) {
  // This might be 'abc'
  console.log(key)
}
```

To reflect the new behavior, `db.supports.snapshots` is now false. If snapshot guarantees are a must for your application then use `iterator.all()` (which is a new method compared to `level-js`) and call it immediately after creating the iterator:

```js
const entries = await db.iterator({ limit: 50 }).all()

// Synchronously iterate the result
for (const [key, value] of entries) {
  console.log(key)
}
```

Unrelated, iterating should now be faster because `browser-level` iterators use the `getAll()` and `getAllKeys()` methods of IndexedDB, instead of a cursor like `level-js` did. This means multiple entries are transferred from IndexedDB to JS in a single turn of the JS event loop, rather than one turn per entry. Reverse iterators do still use a cursor and are therefor slower.

Lastly, the new logic enabled us to implement and fully support `iterator.seek()`.

### Changes to lesser-used properties and methods

- The `db.prefix` property of `level-js`, conflicting with the `db.prefix` property of sublevels, has been renamed to `db.namePrefix`. The relevant constructor option (`prefix`) remains the same.
- The `db.location`, `db.version` and `db.db` properties are now read-only getters
- The internal `db.store()` and `db.await()` methods are no longer accessible
- The `db.upgrade()` utility for upgrading from v4.x to v5.x has been removed.

---

_For earlier releases, before `browser-level` was forked from `level-js` (v6.1.0), please see [the upgrade guide of `level-js`](https://github.com/Level/level-js/blob/HEAD/UPGRADING.md)._
