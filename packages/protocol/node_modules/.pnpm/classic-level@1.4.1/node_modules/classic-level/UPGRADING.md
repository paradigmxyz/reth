# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## 1.0.0

**Introducing `classic-level`: a fork of [`leveldown`](https://github.com/Level/leveldown) that implements the [`abstract-level`](https://github.com/Level/abstract-level) interface instead of [`abstract-leveldown`](https://github.com/Level/abstract-leveldown). It thus has the same API as `level` and `levelup` including encodings, promises and events. In addition, you can now choose to use Uint8Array instead of Buffer. Sublevels are builtin.**

We've put together several upgrade guides for different modules. See the [FAQ](https://github.com/Level/community#faq) to find the best upgrade guide for you. This one describes how to replace `leveldown` with `classic-level`. There will be a separate guide for upgrading `level`.

Support of Node.js 10 has been dropped.

### What is not covered here

If you are using any of the following, please also read the upgrade guide of [`abstract-level@1`](https://github.com/Level/abstract-level/blob/main/UPGRADING.md#100) which goes into more detail about these:

- Specific error messages (replaced with error codes)
- The `db.iterator().end()` method (renamed to `close()`, with `end()` as an alias)
- Zero-length keys and range options (now valid)
- The `db.supports.bufferKeys` property.

### Changes to initialization

We started using classes, which means using `new` is now required. If you previously did:

```js
const leveldown = require('leveldown')
const db = leveldown('./db')
```

You must now do:

```js
const { ClassicLevel } = require('classic-level')
const db = new ClassicLevel('./db')
```

Because `abstract-level` does not require calling `db.open()` before other methods (a feature known as deferred open) it is now preferred to pass options to the constructor. For example:

```js
const db = new ClassicLevel('./db', {
  createIfMissing: false,
  compression: false
})
```

If `db.open(options)` is called manually those options will be shallowly merged with options from the constructor:

```js
// Results in { createIfMissing: false, compression: true }
await db.open({ compression: true })
```

This means that if you were using this `db.open(options)` pattern, it works as before, except if you were _also_ wrapping `leveldown` with `levelup` _and_ passing options to the `levelup` constructor. Because `levelup` would overwrite rather than merge options.

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

### Unwrapping the onion

If you were wrapping `leveldown` with `levelup`, `encoding-down` and / or `subleveldown`, remove those modules. If you previously did:

```js
const leveldown = require('leveldown')
const levelup = require('levelup')
const enc = require('encoding-down')
const subleveldown = require('subleveldown')

const db = levelup(enc(leveldown('./db')))
const sublevel = subleveldown(db, 'foo')
```

You must now do:

```js
const { ClassicLevel } = require('classic-level')
const db = new ClassicLevel('./db')
const sublevel = db.sublevel('foo')
```

### Changes to iterators

- The `highWaterMark` option has been renamed to `highWaterMarkBytes` to remove a conflict with streams. Please see the README for details on this (previously undocumented) option.
- On iterators with `{ keys: false }` or `{ values: false }` options, the key or value is now `undefined` rather than an empty string (this was only the case in `leveldown`).
- The `iterator.cache` and `iterator.finished` properties are no longer accessible. If you were using these then you'll want to checkout the new `nextv()` method in the README.

### Sugar on additional methods

The additional methods `db.approximateSize()` and `db.compactRange()` now support the same patterns as other methods:

- Support of encoding options
- Deferred open (no need to call `db.open()` before these methods)
- Returning a promise if no callback is provided.

---

_For earlier releases, before `classic-level` was forked from `leveldown` (v6.1.0), please see [the upgrade guide of `leveldown`](https://github.com/Level/leveldown/blob/HEAD/UPGRADING.md)._
