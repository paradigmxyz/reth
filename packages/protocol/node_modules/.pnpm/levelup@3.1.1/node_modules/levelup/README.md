# levelup

[![level badge][level-badge]](https://github.com/level/awesome)
[![npm](https://img.shields.io/npm/v/levelup.svg)](https://www.npmjs.com/package/levelup)
![Node version](https://img.shields.io/node/v/levelup.svg)
[![Build Status](https://secure.travis-ci.org/Level/levelup.svg?branch=master)](http://travis-ci.org/Level/levelup)
[![dependencies](https://david-dm.org/Level/levelup.svg)](https://david-dm.org/level/levelup)
[![npm](https://img.shields.io/npm/dm/levelup.svg)](https://www.npmjs.com/package/levelup)
[![Coverage Status](https://coveralls.io/repos/github/Level/levelup/badge.svg)](https://coveralls.io/github/Level/levelup)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)

## Table of Contents

<details><summary>Click to expand</summary>

-   [Introduction](#introduction)
-   [Supported Platforms](#supported-platforms)
-   [Usage](#usage)
-   [API](#api)
-   [Promise Support](#promise-support)
-   [Events](#events)
-   [Extending](#extending)
-   [Multi-process Access](#multi-process-access)
-   [Support](#support)
-   [Contributing](#contributing)
-   [Big Thanks](#big-thanks)
-   [License](#license)

</details>

## Introduction

**Fast and simple storage. A Node.js wrapper for `abstract-leveldown` compliant stores, which follow the characteristics of [LevelDB](https://github.com/google/leveldb).**

LevelDB is a simple key-value store built by Google. It's used in Google Chrome and many other products. LevelDB supports arbitrary byte arrays as both keys and values, singular _get_, _put_ and _delete_ operations, _batched put and delete_, bi-directional iterators and simple compression using the very fast [Snappy](http://google.github.io/snappy/) algorithm.

LevelDB stores entries sorted lexicographically by keys. This makes the [streaming interface](#createReadStream) of `levelup` - which exposes LevelDB iterators as [Readable Streams](https://nodejs.org/docs/latest/api/stream.html#stream_readable_streams) - a very powerful query mechanism.

The most common store is [`leveldown`](https://github.com/level/leveldown/) which provides a pure C++ binding to LevelDB. [Many alternative stores are available](https://github.com/Level/awesome/#stores) such as [`level.js`](https://github.com/level/level.js) in the browser or [`memdown`](https://github.com/level/memdown) for an in-memory store. They typically support strings and Buffers for both keys and values. For a richer set of data types you can wrap the store with [`encoding-down`](https://github.com/level/encoding-down).

**The [`level`](https://github.com/level/level) package is the recommended way to get started.** It conveniently bundles `levelup`, [`leveldown`](https://github.com/level/leveldown/) and [`encoding-down`](https://github.com/level/encoding-down). Its main export is `levelup` - i.e. you can do `var db = require('level')`.

## Supported Platforms

We aim to support Active LTS and Current Node.js releases as well as browsers. For support of the underlying store, please see the respective documentation.

[![Sauce Test Status](https://saucelabs.com/browser-matrix/levelup.svg)](https://saucelabs.com/u/levelup)

## Usage

**If you are upgrading:** please see [`UPGRADING.md`](UPGRADING.md).

First you need to install `levelup`! No stores are included so you must also install `leveldown` (for example).

```sh
$ npm install levelup leveldown
```

All operations are asynchronous. If you do not provide a callback, [a Promise is returned](#promise-support).

```js
var levelup = require('levelup')
var leveldown = require('leveldown')

// 1) Create our store
var db = levelup(leveldown('./mydb'))

// 2) Put a key & value
db.put('name', 'levelup', function (err) {
  if (err) return console.log('Ooops!', err) // some kind of I/O error

  // 3) Fetch by key
  db.get('name', function (err, value) {
    if (err) return console.log('Ooops!', err) // likely the key was not found

    // Ta da!
    console.log('name=' + value)
  })
})
```

## API

-   [<code><b>levelup()</b></code>](#ctor)
-   [<code>db.<b>open()</b></code>](#open)
-   [<code>db.<b>close()</b></code>](#close)
-   [<code>db.<b>put()</b></code>](#put)
-   [<code>db.<b>get()</b></code>](#get)
-   [<code>db.<b>del()</b></code>](#del)
-   [<code>db.<b>batch()</b></code> _(array form)_](#batch)
-   [<code>db.<b>batch()</b></code> _(chained form)_](#batch_chained)
-   [<code>db.<b>isOpen()</b></code>](#isOpen)
-   [<code>db.<b>isClosed()</b></code>](#isClosed)
-   [<code>db.<b>createReadStream()</b></code>](#createReadStream)
-   [<code>db.<b>createKeyStream()</b></code>](#createKeyStream)
-   [<code>db.<b>createValueStream()</b></code>](#createValueStream)
-   [<code>db.<b>iterator()</b></code>](#iterator)

### Special Notes

-   <a href="#writeStreams">What happened to <code><b>db.createWriteStream()</b></code></a>

<a name="ctor"></a>

### `levelup(db[, options[, callback]])`

The main entry point for creating a new `levelup` instance.

-   `db` must be an [`abstract-leveldown`](https://github.com/level/abstract-leveldown) compliant store.
-   `options` is passed on to the underlying store when opened and is specific to the type of store being used

Calling `levelup(db)` will also open the underlying store. This is an asynchronous operation which will trigger your callback if you provide one. The callback should take the form `function (err, db) {}` where `db` is the `levelup` instance. If you don't provide a callback, any read & write operations are simply queued internally until the store is fully opened.

This leads to two alternative ways of managing a `levelup` instance:

```js
levelup(leveldown(location), options, function (err, db) {
  if (err) throw err

  db.get('foo', function (err, value) {
    if (err) return console.log('foo does not exist')
    console.log('got foo =', value)
  })
})
```

Versus the equivalent:

```js
// Will throw if an error occurs
var db = levelup(leveldown(location), options)

db.get('foo', function (err, value) {
  if (err) return console.log('foo does not exist')
  console.log('got foo =', value)
})
```

<a name="open"></a>

### `db.open([callback])`

Opens the underlying store. In general you should never need to call this method directly as it's automatically called by <a href="#ctor"><code>levelup()</code></a>.

However, it is possible to _reopen_ the store after it has been closed with <a href="#close"><code>close()</code></a>, although this is not generally advised.

If no callback is passed, a promise is returned.

<a name="close"></a>

### `db.close([callback])`

<code>close()</code> closes the underlying store. The callback will receive any error encountered during closing as the first argument.

You should always clean up your `levelup` instance by calling `close()` when you no longer need it to free up resources. A store cannot be opened by multiple instances of `levelup` simultaneously.

If no callback is passed, a promise is returned.

<a name="put"></a>

### `db.put(key, value[, options][, callback])`

<code>put()</code> is the primary method for inserting data into the store. Both `key` and `value` can be of any type as far as `levelup` is concerned.

`options` is passed on to the underlying store.

If no callback is passed, a promise is returned.

<a name="get"></a>

### `db.get(key[, options][, callback])`

<code>get()</code> is the primary method for fetching data from the store. The `key` can be of any type. If it doesn't exist in the store then the callback or promise will receive an error. A not-found err object will be of type `'NotFoundError'` so you can `err.type == 'NotFoundError'` or you can perform a truthy test on the property `err.notFound`.

```js
db.get('foo', function (err, value) {
  if (err) {
    if (err.notFound) {
      // handle a 'NotFoundError' here
      return
    }
    // I/O or other error, pass it up the callback chain
    return callback(err)
  }

  // .. handle `value` here
})
```

`options` is passed on to the underlying store.

If no callback is passed, a promise is returned.

<a name="del"></a>

### `db.del(key[, options][, callback])`

<code>del()</code> is the primary method for removing data from the store.

```js
db.del('foo', function (err) {
  if (err)
    // handle I/O or other error
});
```

`options` is passed on to the underlying store.

If no callback is passed, a promise is returned.

<a name="batch"></a>

### `db.batch(array[, options][, callback])` _(array form)_

<code>batch()</code> can be used for very fast bulk-write operations (both _put_ and _delete_). The `array` argument should contain a list of operations to be executed sequentially, although as a whole they are performed as an atomic operation inside the underlying store.

Each operation is contained in an object having the following properties: `type`, `key`, `value`, where the _type_ is either `'put'` or `'del'`. In the case of `'del'` the `value` property is ignored. Any entries with a `key` of `null` or `undefined` will cause an error to be returned on the `callback` and any `type: 'put'` entry with a `value` of `null` or `undefined` will return an error.

```js
var ops = [
  { type: 'del', key: 'father' },
  { type: 'put', key: 'name', value: 'Yuri Irsenovich Kim' },
  { type: 'put', key: 'dob', value: '16 February 1941' },
  { type: 'put', key: 'spouse', value: 'Kim Young-sook' },
  { type: 'put', key: 'occupation', value: 'Clown' }
]

db.batch(ops, function (err) {
  if (err) return console.log('Ooops!', err)
  console.log('Great success dear leader!')
})
```

`options` is passed on to the underlying store.

If no callback is passed, a promise is returned.

<a name="batch_chained"></a>

### `db.batch()` _(chained form)_

<code>batch()</code>, when called with no arguments will return a `Batch` object which can be used to build, and eventually commit, an atomic batch operation. Depending on how it's used, it is possible to obtain greater performance when using the chained form of `batch()` over the array form.

```js
db.batch()
  .del('father')
  .put('name', 'Yuri Irsenovich Kim')
  .put('dob', '16 February 1941')
  .put('spouse', 'Kim Young-sook')
  .put('occupation', 'Clown')
  .write(function () { console.log('Done!') })
```

<b><code>batch.put(key, value)</code></b>

Queue a _put_ operation on the current batch, not committed until a `write()` is called on the batch.

This method may `throw` a `WriteError` if there is a problem with your put (such as the `value` being `null` or `undefined`).

<b><code>batch.del(key)</code></b>

Queue a _del_ operation on the current batch, not committed until a `write()` is called on the batch.

This method may `throw` a `WriteError` if there is a problem with your delete.

<b><code>batch.clear()</code></b>

Clear all queued operations on the current batch, any previous operations will be discarded.

<b><code>batch.length</code></b>

The number of queued operations on the current batch.

<b><code>batch.write(\[options][, callback])</code></b>

Commit the queued operations for this batch. All operations not _cleared_ will be written to the underlying store atomically, that is, they will either all succeed or fail with no partial commits.

The optional `options` object is passed to the `.write()` operation of the underlying batch object.

If no callback is passed, a promise is returned.

<a name="isOpen"></a>

### `db.isOpen()`

A `levelup` instance can be in one of the following states:

-   _"new"_     - newly created, not opened or closed
-   _"opening"_ - waiting for the underlying store to be opened
-   _"open"_    - successfully opened the store, available for use
-   _"closing"_ - waiting for the store to be closed
-   _"closed"_  - store has been successfully closed, should not be used

`isOpen()` will return `true` only when the state is "open".

<a name="isClosed"></a>

### `db.isClosed()`

_See <a href="#put"><code>isOpen()</code></a>_

`isClosed()` will return `true` only when the state is "closing" _or_ "closed", it can be useful for determining if read and write operations are permissible.

<a name="createReadStream"></a>

### `db.createReadStream([options])`

Returns a [Readable Stream](https://nodejs.org/docs/latest/api/stream.html#stream_readable_streams) of key-value pairs. A pair is an object with `key` and `value` properties. By default it will stream all entries in the underlying store from start to end. Use the options described below to control the range, direction and results.

```js
db.createReadStream()
  .on('data', function (data) {
    console.log(data.key, '=', data.value)
  })
  .on('error', function (err) {
    console.log('Oh my!', err)
  })
  .on('close', function () {
    console.log('Stream closed')
  })
  .on('end', function () {
    console.log('Stream ended')
  })
```

You can supply an options object as the first parameter to `createReadStream()` with the following properties:

-   `gt` (greater than), `gte` (greater than or equal) define the lower bound of the range to be streamed. Only entries where the key is greater than (or equal to) this option will be included in the range. When `reverse=true` the order will be reversed, but the entries streamed will be the same.

-   `lt` (less than), `lte` (less than or equal) define the higher bound of the range to be streamed. Only entries where the key is less than (or equal to) this option will be included in the range. When `reverse=true` the order will be reversed, but the entries streamed will be the same.

-   `reverse` _(boolean, default: `false`)_: stream entries in reverse order. Beware that due to the way that stores like LevelDB work, a reverse seek can be slower than a forward seek.

-   `limit` _(number, default: `-1`)_: limit the number of entries collected by this stream. This number represents a _maximum_ number of entries and may not be reached if you get to the end of the range first. A value of `-1` means there is no limit. When `reverse=true` the entries with the highest keys will be returned instead of the lowest keys.

-   `keys` _(boolean, default: `true`)_: whether the results should contain keys. If set to `true` and `values` set to `false` then results will simply be keys, rather than objects with a `key` property. Used internally by the `createKeyStream()` method.

-   `values` _(boolean, default: `true`)_: whether the results should contain values. If set to `true` and `keys` set to `false` then results will simply be values, rather than objects with a `value` property. Used internally by the `createValueStream()` method.

Legacy options:

-   `start`: instead use `gte`

-   `end`: instead use `lte`

<a name="createKeyStream"></a>

### `db.createKeyStream([options])`

Returns a [Readable Stream](https://nodejs.org/docs/latest/api/stream.html#stream_readable_streams) of keys rather than key-value pairs. Use the same options as described for [`createReadStream`](#createReadStream) to control the range and direction.

You can also obtain this stream by passing an options object to `createReadStream()` with `keys` set to `true` and `values` set to `false`. The result is equivalent; both streams operate in [object mode](https://nodejs.org/docs/latest/api/stream.html#stream_object_mode).

```js
db.createKeyStream()
  .on('data', function (data) {
    console.log('key=', data)
  })

// same as:
db.createReadStream({ keys: true, values: false })
  .on('data', function (data) {
    console.log('key=', data)
  })
```

<a name="createValueStream"></a>

### `db.createValueStream([options])`

Returns a [Readable Stream](https://nodejs.org/docs/latest/api/stream.html#stream_readable_streams) of values rather than key-value pairs. Use the same options as described for [`createReadStream`](#createReadStream) to control the range and direction.

You can also obtain this stream by passing an options object to `createReadStream()` with `values` set to `true` and `keys` set to `false`. The result is equivalent; both streams operate in [object mode](https://nodejs.org/docs/latest/api/stream.html#stream_object_mode).

```js
db.createValueStream()
  .on('data', function (data) {
    console.log('value=', data)
  })

// same as:
db.createReadStream({ keys: false, values: true })
  .on('data', function (data) {
    console.log('value=', data)
  })
```

<a name="iterator"></a>

### `db.iterator([options])`

Returns an [`abstract-leveldown` iterator](https://github.com/Level/abstract-leveldown/#abstractleveldown_iteratoroptions), which is what powers the readable streams above. Options are the same as the range options of [`createReadStream()`](#createReadStream) and are passed to the underlying store.

<a name="writeStreams"></a>

#### What happened to `db.createWriteStream`?

`db.createWriteStream()` has been removed in order to provide a smaller and more maintainable core. It primarily existed to create symmetry with `db.createReadStream()` but through much [discussion](https://github.com/level/levelup/issues/199), removing it was the best course of action.

The main driver for this was performance. While `db.createReadStream()` performs well under most use cases, `db.createWriteStream()` was highly dependent on the application keys and values. Thus we can't provide a standard implementation and encourage more `write-stream` implementations to be created to solve the broad spectrum of use cases.

Check out the implementations that the community has already produced [here](https://github.com/level/levelup/wiki/Modules#write-streams).

## Promise Support

`levelup` ships with native `Promise` support out of the box.

Each function accepting a callback returns a promise if the callback is omitted. This applies for:

-   `db.get(key[, options])`
-   `db.put(key, value[, options])`
-   `db.del(key[, options])`
-   `db.batch(ops[, options])`
-   `db.batch().write()`

The only exception is the `levelup` constructor itself, which if no callback is passed will lazily open the underlying store in the background.

Example:

```js
var db = levelup(leveldown('./my-db'))

db.put('foo', 'bar')
  .then(function () { return db.get('foo') })
  .then(function (value) { console.log(value) })
  .catch(function (err) { console.error(err) })
```

Or using `async/await`:

```js
const main = async () => {
  const db = levelup(leveldown('./my-db'))

  await db.put('foo', 'bar')
  console.log(await db.get('foo'))
}
```

## Events

`levelup` is an [`EventEmitter`](https://nodejs.org/api/events.html) and emits the following events.

| Event     | Description                 | Arguments            |
| :-------- | :-------------------------- | :------------------- |
| `put`     | Key has been updated        | `key, value` (any)   |
| `del`     | Key has been deleted        | `key` (any)          |
| `batch`   | Batch has executed          | `operations` (array) |
| `opening` | Underlying store is opening | -                    |
| `open`    | Store has opened            | -                    |
| `ready`   | Alias of `open`             | -                    |
| `closing` | Store is closing            | -                    |
| `closed`  | Store has closed.           | -                    |

For example you can do:

```js
db.on('put', function (key, value) {
  console.log('inserted', { key, value })
})
```

## Extending

A list of <a href="https://github.com/level/levelup/wiki/Modules"><b>Level modules and projects</b></a> can be found in the wiki. We are in the process of moving all this to [`awesome`](https://github.com/Level/awesome/).

## Multi-process Access

Stores like LevelDB are thread-safe but they are **not** suitable for accessing with multiple processes. You should only ever have a store open from a single Node.js process. Node.js clusters are made up of multiple processes so a `levelup` instance cannot be shared between them either.

See the aformentioned <a href="https://github.com/level/levelup/wiki/Modules"><b>wiki</b></a> for modules like [multilevel](https://github.com/juliangruber/multilevel), that may help if you require a single store to be shared across processes.

## Support

There are multiple ways you can find help in using Level in Node.js:

-   **IRC:** you'll find an active group of `levelup` users in the **##leveldb** channel on Freenode, including most of the contributors to this project.
-   **Mailing list:** there is an active [Node.js LevelDB](https://groups.google.com/forum/#!forum/node-levelup) Google Group.
-   **GitHub:** you're welcome to open an issue here on this GitHub repository if you have a question.

## Contributing

`levelup` is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [contribution guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Big Thanks

Cross-browser Testing Platform and Open Source ♥ Provided by [Sauce Labs](https://saucelabs.com).

[![Sauce Labs logo](./sauce-labs.svg)](https://saucelabs.com)

## License

[MIT](LICENSE.md) © 2012-present `levelup` [Contributors](CONTRIBUTORS.md).

[level-badge]: http://leveldb.org/img/badge.svg
