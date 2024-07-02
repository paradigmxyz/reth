# encoding-down

> An [`abstract-leveldown`] implementation that wraps another store to encode keys and values.

[![level badge][level-badge]](https://github.com/level/awesome)
[![npm](https://img.shields.io/npm/v/encoding-down.svg)](https://www.npmjs.com/package/encoding-down)
![Node version](https://img.shields.io/node/v/encoding-down.svg)
[![Travis](https://travis-ci.org/Level/encoding-down.svg?branch=master)](https://travis-ci.org/Level/encoding-down)
[![david](https://david-dm.org/Level/encoding-down.svg)](https://david-dm.org/level/encoding-down)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)
[![npm](https://img.shields.io/npm/dm/encoding-down.svg)](https://www.npmjs.com/package/encoding-down)

## Introduction

Stores like [`leveldown`] can only store strings and Buffers. For a richer set of data types you can wrap such a store with `encoding-down`. It allows you to specify an _encoding_ to use for keys and values independently. This not only widens the range of input types, but also limits the range of output types. The encoding is applied to all read and write operations: it encodes writes and decodes reads.

[Many encodings are builtin][builtin-encodings] courtesy of [`level-codec`]. The default encoding is `utf8` which ensures you'll always get back a string. You can also provide a custom encoding like `bytewise` - [or your own](#custom-encodings)!

## Usage

Without any options, `encoding-down` defaults to the `utf8` encoding.

```js
const levelup = require('levelup')
const leveldown = require('leveldown')
const encode = require('encoding-down')

const db = levelup(encode(leveldown('./db1')))

db.put('example', Buffer.from('encoding-down'), function (err) {
  db.get('example', function (err, value) {
    console.log(typeof value, value) // 'string encoding-down'
  })
})
```

Can we store objects? Yes!

```js
const db = levelup(encode(leveldown('./db2'), { valueEncoding: 'json' }))

db.put('example', { awesome: true }, function (err) {
  db.get('example', function (err, value) {
    console.log(value) // { awesome: true }
    console.log(typeof value) // 'object'
  })
})
```

How about storing Buffers, but getting back a hex-encoded string?

```js
const db = levelup(encode(leveldown('./db3'), { valueEncoding: 'hex' }))

db.put('example', Buffer.from([0, 255]), function (err) {
  db.get('example', function (err, value) {
    console.log(typeof value, value) // 'string 00ff'
  })
})
```

What if we previously stored binary data?

```js
const db = levelup(encode(leveldown('./db4'), { valueEncoding: 'binary' }))

db.put('example', Buffer.from([0, 255]), function (err) {
  db.get('example', function (err, value) {
    console.log(typeof value, value) // 'object <Buffer 00 ff>'
  })

  // Override the encoding for this operation
  db.get('example', { valueEncoding: 'base64' }, function (err, value) {
    console.log(typeof value, value) // 'string AP8='
  })
})
```

And what about keys?

```js
const db = levelup(encode(leveldown('./db5'), { keyEncoding: 'json' }))

db.put({ awesome: true }, 'example', function (err) {
  db.get({ awesome: true }, function (err, value) {
    console.log(value) // 'example'
  })
})
```

```js
const db = levelup(encode(leveldown('./db6'), { keyEncoding: 'binary' }))

db.put(Buffer.from([0, 255]), 'example', function (err) {
  db.get('00ff', { keyEncoding: 'hex' }, function (err, value) {
    console.log(value) // 'example'
  })
})
```

## Usage with [`level`]

The [`level`] module conveniently bundles `encoding-down` and passes its `options` to `encoding-down`. This means you can simply do:

```js
const level = require('level')
const db = level('./db7', { valueEncoding: 'json' })

db.put('example', 42, function (err) {
  db.get('example', function (err, value) {
    console.log(value) // 42
    console.log(typeof value) // 'number'
  })
})
```

## API

### `const db = require('encoding-down')(db[, options])`

-   `db` must be an [`abstract-leveldown`] compliant store
-   `options` are passed to [`level-codec`]&#x3A;
    -   `keyEncoding`: encoding to use for keys
    -   `valueEncoding`: encoding to use for values

Both encodings default to `'utf8'`. They can be a string (builtin `level-codec` encoding) or an object (custom encoding).

## Custom encodings

Please refer to [`level-codec` documentation][encoding-format] for a precise description of the format. Here's a quick example with `level` and `async/await` just for fun:

```js
const level = require('level')
const lexint = require('lexicographic-integer')

async function main () {
  const db = level('./db8', {
    keyEncoding: {
      type: 'lexicographic-integer',
      encode: (n) => lexint.pack(n, 'hex'),
      decode: lexint.unpack,
      buffer: false
    }
  })

  await db.put(2, 'example')
  await db.put(10, 'example')

  // Without our encoding, the keys would sort as 10, 2.
  db.createKeyStream().on('data', console.log) // 2, 10
}

main()
```

With an npm-installed encoding (modularity ftw!) we can reduce the above to:

```js
const level = require('level')
const lexint = require('lexicographic-integer-encoding')('hex')

const db = level('./db8', {
  keyEncoding: lexint
})
```

## License

[MIT](./LICENSE.md) © 2017-present `encoding-down` [Contributors](./CONTRIBUTORS.md).

[level-badge]: http://leveldb.org/img/badge.svg

[`abstract-leveldown`]: https://github.com/level/abstract-leveldown

[`leveldown`]: https://github.com/level/leveldown

[`level`]: https://github.com/level/level

[`level-codec`]: https://github.com/level/codec

[builtin-encodings]: https://github.com/level/codec#builtin-encodings

[encoding-format]: https://github.com/level/codec#encoding-format
