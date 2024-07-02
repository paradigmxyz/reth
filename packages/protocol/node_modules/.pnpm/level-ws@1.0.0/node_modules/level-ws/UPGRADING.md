# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## v1

Dropped support for node 0.10, 0.12, 2, 3, 4 and 5.

------------------------------------

The API only exports a single function and no longer patches `levelup`.

So if you previously did:

```js
var ws = require('level-ws')
var db = ws(level('DB'))
var stream = db.createWriteStream()
```

You should now do:

```js
var WriteStream = require('level-ws')
var db = level('DB')
var stream = WriteStream(db)
```

------------------------------------

The parameters to the stream constructor were flipped.

So if you previously did:

```js
var WriteStream = require('level-ws').WriteStream
var db = level('DB')
var stream = WriteStream({ type: 'del' }, db)
```

You should now do:

```js
var WriteStream = require('level-ws')
var db = level('DB')
var stream = WriteStream(db, { type: 'del' })
```

------------------------------------

The behavior of `maxBufferLength` was changed. Previously all write operations exceeding `maxBufferLength` in the same tick were dropped. Instead the stream is now paused until a batch has been flushed and unpaused once the batch has been completed.

------------------------------------

`WriteStream#destroySoon()` was removed.

------------------------------------

Internally `this.writable` and `this.readable` were removed. However, `this.writable` still exists due to inheritance, but `this.readable` is now `undefined`.

------------------------------------

Default `'utf8'` encoding was removed and also per stream encodings. However, it's still possible to specify encodings for individual entries. This means if you previously relied on per stream encodings, you must specify this in calls to `.write()`:

```js
writeStream.write({
  key: new Buffer([1, 2, 3]),
  value: { some: 'json' },
  keyEncoding: 'binary',
  valueEncoding : 'json'
})
```
