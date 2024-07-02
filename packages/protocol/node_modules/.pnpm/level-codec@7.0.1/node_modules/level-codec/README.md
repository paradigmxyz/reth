
# level-codec

<img alt="LevelDB Logo" height="100" src="http://leveldb.org/img/logo.svg">

**[LevelUP's](https://github.com/rvagg/node-levelup) encoding logic.**

[![Build Status](https://travis-ci.org/Level/codec.png)](https://travis-ci.org/Level/codec)
[![Greenkeeper badge](https://badges.greenkeeper.io/Level/codec.svg)](https://greenkeeper.io/)

## API

### Codec([opts])

  Create a new codec, with a global options object.

  This could be something like

```js
var codec = new Codec(db.options);
```

### #encodeKey(key[, opts])

  Encode `key` with given `opts`.

### #encodeValue(value[, opts])

  Encode `value` with given `opts`.

### #encodeBatch(batch[, opts])

  Encode `batch` ops with given `opts`.

### #encodeLtgt(ltgt)

  Encode the ltgt values of option object `ltgt`.

### #decodeKey(key[, opts])

  Decode `key` with given `opts`.

### #decodeValue(value[, opts])

  Decode `value` with given `opts`.

### #createStreamDecoder([opts])

  Create a function with signature `(key, value)`, that for each key/value pair returned from a levelup read stream returns the decoded value to be emitted.

### #keyAsBuffer([opts])

  Check whether `opts` and the global `opts` call for a binary key encoding.

### #valueAsBuffer([opts])

  Check whether `opts` and the global `opts` call for a binary value encoding.

### #encodings

  The supported encodings as object of form

```js
{
  "name": {
    "encode": Function,
    "decode": Function,
    "buffer": Boolean,
    "type": String
  }
}
```

  Currently supported encodings:

  - utf8
  - json
  - binary
  - hex
  - ascii
  - base64
  - ucs2
  - ucs-2
  - utf16le
  - utf-16le
  - none (bypass level-codec)

## Publishers

* [@juliangruber](https://github.com/juliangruber)
* [@ralphtheninja](https://github.com/ralphtheninja)

## License &amp; copyright

Copyright (c) 2012-2015 LevelUP contributors.

LevelUP is licensed under the MIT license. All rights not explicitly granted in the MIT license are reserved. See the included LICENSE.md file for more details.
