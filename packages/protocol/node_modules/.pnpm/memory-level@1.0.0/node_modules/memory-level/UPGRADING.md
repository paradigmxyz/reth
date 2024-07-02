# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [`CHANGELOG`](CHANGELOG.md).

## 1.0.0

**Introducing `memory-level`: a fork of [`memdown`](https://github.com/Level/memdown) that removes the need for [`level-mem`](https://github.com/Level/mem), [`levelup`](https://github.com/Level/levelup) and more. It implements the [`abstract-level`](https://github.com/Level/abstract-level) interface instead of [`abstract-leveldown`](https://github.com/Level/abstract-leveldown) and thus has the same API as `level-mem` and `levelup` including encodings, promises and events. In addition, you can now choose to use Uint8Array instead of Buffer. Sublevels are builtin.**

We've put together several upgrade guides for different modules. See the [FAQ](https://github.com/Level/community#faq) to find the best upgrade guide for you. This upgrade guide describes how to replace `memdown` or `level-mem` with `memory-level`. If you are using any of the following, please also read the upgrade guide of [`abstract-level@1`](https://github.com/Level/abstract-level/blob/main/UPGRADING.md#100) which goes into more detail about these:

- Specific error messages (replaced with error codes)
- The callback argument of the constructor (gone)
- The `'binary'` encoding (renamed to `'buffer'`, with `'binary'` as an alias)
- The `db.iterator().end()` method (renamed to `close()`, with `end()` as an alias)
- Zero-length keys and range options (now valid)
- The `'ascii'`, `'ucs2'`, `'utf16le'` and `'id'` encodings (gone)
- The undocumented `encoding` alias for the `valueEncoding` option (gone)
- The `db.supports.bufferKeys` property.

Support of Node.js 10 has been dropped.

### Upgrade from `level-mem` to `memory-level`

Using `new` is now required. If you previously did:

```js
const mem = require('level-mem')
const db1 = mem()
const db2 = mem({ valueEncoding: 'json' })
```

You must now do:

```js
const { MemoryLevel } = require('memory-level')
const db1 = new MemoryLevel()
const db2 = new MemoryLevel({ valueEncoding: 'json' })
```

Node.js readable streams must now be created with a new standalone module called [`level-read-stream`](https://github.com/Level/read-stream), rather than database methods like `db.createReadStream()`. Please see its [upgrade guide](https://github.com/Level/read-stream/blob/main/UPGRADING.md#100) for details.

### Upgrade from `memdown` to `memory-level`

_This section is only relevant if you're using `memdown` directly, rather than as a transitive dependency of `level-mem`._

The `asBuffer`, `valueAsBuffer` and `keyAsBuffer` options have been replaced with encoding options. The default encoding is `'utf8'` which means operations return strings rather than Buffers by default. If you previously did:

```js
const memdown = require('memdown')
const db = memdown()

db.get('example', { asBuffer: false }, callback)
db.get('example', callback)
```

You must now do:

```js
const { MemoryLevel } = require('memory-level')
const db = new MemoryLevel()

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

If you were wrapping `memdown` with `levelup`, `encoding-down` and / or `subleveldown`, remove those modules. If you previously did:

```js
const memdown = require('memdown')
const levelup = require('levelup')
const enc = require('encoding-down')
const subleveldown = require('subleveldown')

const db = levelup(enc(memdown()))
const sublevel = subleveldown(db, 'foo')
```

You must now do:

```js
const { MemoryLevel } = require('memory-level')
const db = new MemoryLevel()
const sublevel = db.sublevel('foo')
```

Lastly, private properties like `_store` (unlikely used externally) are no longer accessible.

---

_For earlier releases, before `memory-level` was forked from `memdown` (v6.1.1), please see [the upgrade guide of `memdown`](https://github.com/Level/memdown/blob/HEAD/UPGRADING.md)._
