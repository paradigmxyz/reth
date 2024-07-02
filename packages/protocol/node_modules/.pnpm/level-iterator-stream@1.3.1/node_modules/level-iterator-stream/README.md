
# level-iterator-stream

<img alt="LevelDB Logo" height="100" src="http://leveldb.org/img/logo.svg">

**Turn a leveldown iterator into a readable stream**

[![Build Status](https://travis-ci.org/Level/iterator-stream.png)](https://travis-ci.org/Level/iterator-stream)

## Example

```js
var iteratorStream = require('level-iterator-stream');
var leveldown = require('leveldown');

var db = leveldown(__dirname + '/db');
db.open(function(err){
  if (err) throw err;

  var stream = iteratorStream(db.iterator());
  stream.on('data', function(kv){
    console.log('%s -> %s', kv.key, kv.value);
  });
});
```

## Installation

```bash
$ npm install level-iterator-stream
```

## API

### iteratorStream(iterator[, options])

  Create a readable stream from `iterator`. `options` are passed down to the
  `require('readable-stream').Readable` constructor, with `objectMode` forced
  to `true`.

  If `options.decoder` is passed, each key/value pair will be transformed by it.
  Otherwise, an object with `{ key, value }` will be emitted.

  When the stream ends, the `iterator` will be closed and afterwards a
  `"close"` event emitted.

  `.destroy()` will force close the underlying iterator.

## Publishers

* [@juliangruber](https://github.com/juliangruber)

## License &amp; copyright

Copyright (c) 2012-2015 LevelUP contributors.

LevelUP is licensed under the MIT license. All rights not explicitly granted in the MIT license are reserved. See the included LICENSE.md file for more details.
