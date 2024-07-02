# level-ws

> A basic WriteStream implementation for [levelup](https://github.com/level/levelup)

[![level badge][level-badge]](https://github.com/level/awesome)
[![npm](https://img.shields.io/npm/v/level-ws.svg)](https://www.npmjs.com/package/level-ws)
![Node version](https://img.shields.io/node/v/level-ws.svg)
[![Build Status](https://img.shields.io/travis/Level/level-ws.svg)](http://travis-ci.org/Level/level-ws)
[![dependencies](https://david-dm.org/Level/level-ws.svg)](https://david-dm.org/level/level-ws)
[![npm](https://img.shields.io/npm/dm/level-ws.svg)](https://www.npmjs.com/package/level-ws)
[![Coverage Status](https://coveralls.io/repos/github/Level/level-ws/badge.svg)](https://coveralls.io/github/Level/level-ws)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)

`level-ws` provides the most basic general-case WriteStream for `levelup`. It was extracted from the core `levelup` at version 0.18.0.

`level-ws` is not a high-performance WriteStream. If your benchmarking shows that your particular usage pattern and data types do not perform well with this WriteStream then you should try one of the alternative WriteStreams available for `levelup` that are optimised for different use-cases.

**If you are upgrading:** please see [`UPGRADING.md`](UPGRADING.md).

## Usage

```js
var level = require('level')
var WriteStream = require('level-ws')

var db = level('/path/to/db')
var ws = WriteStream(db) // ...
```

## API

### `ws = WriteStream(db[, options])`

Creates a [Writable](https://nodejs.org/dist/latest-v8.x/docs/api/stream.html#stream_class_stream_writable) stream which operates in **objectMode**, accepting objects with `'key'` and `'value'` pairs on its `write()` method.

The optional `options` argument may contain:

* `type` *(string, default: `'put'`)*: Default batch operation for missing `type` property during `ws.write()`.

The WriteStream will buffer writes and submit them as a `batch()` operations where writes occur *within the same tick*.

```js
var ws = WriteStream(db)

ws.on('error', function (err) {
  console.log('Oh my!', err)
})
ws.on('close', function () {
  console.log('Stream closed')
})

ws.write({ key: 'name', value: 'Yuri Irsenovich Kim' })
ws.write({ key: 'dob', value: '16 February 1941' })
ws.write({ key: 'spouse', value: 'Kim Young-sook' })
ws.write({ key: 'occupation', value: 'Clown' })
ws.end()
```

The standard `write()`, `end()` and `destroy()` methods are implemented on the WriteStream. `'drain'`, `'error'`, `'close'` and `'pipe'` events are emitted.

You can specify encodings for individual entries by setting `.keyEncoding` and/or `.valueEncoding`:

```js
writeStream.write({
  key: new Buffer([1, 2, 3]),
  value: { some: 'json' },
  keyEncoding: 'binary',
  valueEncoding : 'json'
})
```

If individual `write()` operations are performed with a `'type'` property of `'del'`, they will be passed on as `'del'` operations to the batch.

```js
var ws = WriteStream(db)

ws.on('error', function (err) {
  console.log('Oh my!', err)
})
ws.on('close', function () {
  console.log('Stream closed')
})

ws.write({ type: 'del', key: 'name' })
ws.write({ type: 'del', key: 'dob' })
ws.write({ type: 'put', key: 'spouse' })
ws.write({ type: 'del', key: 'occupation' })
ws.end()
```

If the *WriteStream* is created with a `'type'` option of `'del'`, all `write()` operations will be interpreted as `'del'`, unless explicitly specified as `'put'`.

```js
var ws = WriteStream(db, { type: 'del' })

ws.on('error', function (err) {
  console.log('Oh my!', err)
})
ws.on('close', function () {
  console.log('Stream closed')
})

ws.write({ key: 'name' })
ws.write({ key: 'dob' })
// but it can be overridden
ws.write({ type: 'put', key: 'spouse', value: 'Ri Sol-ju' })
ws.write({ key: 'occupation' })
ws.end()
```

## License

[MIT](./LICENSE.md) © 2012-present `level-ws` [Contributors](./CONTRIBUTORS.md).

[level-badge]: http://leveldb.org/img/badge.svg
