level-ws
========

![LevelDB Logo](https://twimg0-a.akamaihd.net/profile_images/3360574989/92fc472928b444980408147e5e5db2fa_bigger.png)

**A basic WriteStream implementation for [LevelUP](https://github.com/rvagg/node-levelup)**

[![NPM](https://nodei.co/npm/level-ws.png?downloads)](https://nodei.co/npm/level-ws/)

**level-ws** provides the most basic general-case WriteStream for LevelUP. It was extracted from the core LevelUP at version 0.18.0 but is bundled with [level](https://github.com/Level/level) and similar packages as it provides a general symmetry to the ReadStream in LevelUP.

**level-ws** is not a high-performance WriteStream, if your benchmarking shows that your particular usage pattern and data types do not perform well with this WriteStream then you should try one of the alternative WriteStreams available for LevelUP that are optimised for different use-cases.

## Alternative WriteStream packages

***TODO***

## Usage

To use **level-ws** you simply need to wrap a LevelUP instance and you get a `createWriteStream()` method on it.

```js
var level = require('level')
var levelws = require('level-ws')
var db = level('/path/to/db')

db = levelws(db)
db.createWriteStream() // ...
```

### db.createWriteStream([options])

A **WriteStream** can be obtained by calling the `createWriteStream()` method. The resulting stream is a Node.js **streams2** [Writable](http://nodejs.org/docs/latest/api/stream.html#stream_class_stream_writable_1) which operates in **objectMode**, accepting objects with `'key'` and `'value'` pairs on its `write()` method.

The WriteStream will buffer writes and submit them as a `batch()` operations where writes occur *within the same tick*.

```js
var ws = db.createWriteStream()

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

The standard `write()`, `end()`, `destroy()` and `destroySoon()` methods are implemented on the WriteStream. `'drain'`, `'error'`, `'close'` and `'pipe'` events are emitted.

You can specify encodings both for the whole stream and individual entries:

To set the encoding for the whole stream, provide an options object as the first parameter to `createWriteStream()` with `'keyEncoding'` and/or `'valueEncoding'`.

To set the encoding for an individual entry:

```js
writeStream.write({
    key           : new Buffer([1, 2, 3])
  , value         : { some: 'json' }
  , keyEncoding   : 'binary'
  , valueEncoding : 'json'
})
```

#### write({ type: 'put' })

If individual `write()` operations are performed with a `'type'` property of `'del'`, they will be passed on as `'del'` operations to the batch.

```js
var ws = db.createWriteStream()

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

#### db.createWriteStream({ type: 'del' })

If the *WriteStream* is created with a `'type'` option of `'del'`, all `write()` operations will be interpreted as `'del'`, unless explicitly specified as `'put'`.

```js
var ws = db.createWriteStream({ type: 'del' })

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


### Contributors

**level-ws** is only possible due to the excellent work of the following contributors:

<table><tbody>
<tr><th align="left">Rod Vagg</th><td><a href="https://github.com/rvagg">GitHub/rvagg</a></td><td><a href="http://twitter.com/rvagg">Twitter/@rvagg</a></td></tr>
<tr><th align="left">John Chesley</th><td><a href="https://github.com/chesles/">GitHub/chesles</a></td><td><a href="http://twitter.com/chesles">Twitter/@chesles</a></td></tr>
<tr><th align="left">Jake Verbaten</th><td><a href="https://github.com/raynos">GitHub/raynos</a></td><td><a href="http://twitter.com/raynos2">Twitter/@raynos2</a></td></tr>
<tr><th align="left">Dominic Tarr</th><td><a href="https://github.com/dominictarr">GitHub/dominictarr</a></td><td><a href="http://twitter.com/dominictarr">Twitter/@dominictarr</a></td></tr>
<tr><th align="left">Max Ogden</th><td><a href="https://github.com/maxogden">GitHub/maxogden</a></td><td><a href="http://twitter.com/maxogden">Twitter/@maxogden</a></td></tr>
<tr><th align="left">Lars-Magnus Skog</th><td><a href="https://github.com/ralphtheninja">GitHub/ralphtheninja</a></td><td><a href="http://twitter.com/ralphtheninja">Twitter/@ralphtheninja</a></td></tr>
<tr><th align="left">David Björklund</th><td><a href="https://github.com/kesla">GitHub/kesla</a></td><td><a href="http://twitter.com/david_bjorklund">Twitter/@david_bjorklund</a></td></tr>
<tr><th align="left">Julian Gruber</th><td><a href="https://github.com/juliangruber">GitHub/juliangruber</a></td><td><a href="http://twitter.com/juliangruber">Twitter/@juliangruber</a></td></tr>
<tr><th align="left">Paolo Fragomeni</th><td><a href="https://github.com/hij1nx">GitHub/hij1nx</a></td><td><a href="http://twitter.com/hij1nx">Twitter/@hij1nx</a></td></tr>
<tr><th align="left">Anton Whalley</th><td><a href="https://github.com/No9">GitHub/No9</a></td><td><a href="https://twitter.com/antonwhalley">Twitter/@antonwhalley</a></td></tr>
<tr><th align="left">Matteo Collina</th><td><a href="https://github.com/mcollina">GitHub/mcollina</a></td><td><a href="https://twitter.com/matteocollina">Twitter/@matteocollina</a></td></tr>
<tr><th align="left">Pedro Teixeira</th><td><a href="https://github.com/pgte">GitHub/pgte</a></td><td><a href="https://twitter.com/pgte">Twitter/@pgte</a></td></tr>
<tr><th align="left">James Halliday</th><td><a href="https://github.com/substack">GitHub/substack</a></td><td><a href="https://twitter.com/substack">Twitter/@substack</a></td></tr>
</tbody></table>

<a name="licence"></a>
Licence &amp; copyright
-------------------

Copyright (c) 2012-2013 **level-ws** contributors (listed above).

**level-ws** is licensed under an MIT +no-false-attribs license. All rights not explicitly granted in the MIT license are reserved. See the included LICENSE file for more details.
