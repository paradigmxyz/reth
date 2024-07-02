# browser-level

**An [`abstract-level`][abstract-level] database for browsers, backed by [IndexedDB][indexeddb].** The successor to [`level-js`](https://github.com/Level/level-js). If you are upgrading, please see [UPGRADING.md](UPGRADING.md).

> :pushpin: Which module should I use? What is `abstract-level`? Head over to the [FAQ](https://github.com/Level/community#faq).

[![level badge][level-badge]][awesome]
[![npm](https://img.shields.io/npm/v/browser-level.svg)](https://www.npmjs.com/package/browser-level)
[![Test](https://img.shields.io/github/workflow/status/Level/browser-level/Test?label=test)](https://github.com/Level/browser-level/actions/workflows/test.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/browser-level?label=\&logo=codecov\&logoColor=fff)](https://codecov.io/gh/Level/browser-level)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript\&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective\&logoColor=fff)](https://opencollective.com/level)

## Table of Contents

<details><summary>Click to expand</summary>

- [Usage](#usage)
- [API](#api)
  - [`db = new BrowserLevel(location[, options])`](#db--new-browserlevellocation-options)
  - [`BrowserLevel.destroy(location[, prefix][, callback])`](#browserleveldestroylocation-prefix-callback)
- [Install](#install)
- [Contributing](#contributing)
- [Donate](#donate)
- [License](#license)

</details>

## Usage

```js
const { BrowserLevel } = require('browser-level')

// Create a database called 'example'
const db = new BrowserLevel('example', { valueEncoding: 'json' })

// Add an entry with key 'a' and value 1
await db.put('a', 1)

// Add multiple entries
await db.batch([{ type: 'put', key: 'b', value: 2 }])

// Get value of key 'a': 1
const value = await db.get('a')

// Iterate entries with keys that are greater than 'a'
for await (const [key, value] of db.iterator({ gt: 'a' })) {
  console.log(value) // 2
}
```

<!-- ## Browser Support -->

<!-- [![Sauce Test Status](https://app.saucelabs.com/browser-matrix/level-js.svg)](https://app.saucelabs.com/u/level-js) -->

## API

The API of `browser-level` follows that of [`abstract-level`](https://github.com/Level/abstract-level) with just two additional constructor options (see below) and one additional method (see below). As such, the majority of the API is documented in `abstract-level`. The `createIfMissing` and `errorIfExists` options of `abstract-level` are not supported here.

Like other implementations of `abstract-level`, `browser-level` has first-class support of binary keys and values, using either [Uint8Array](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Uint8Array) or [Buffer](https://nodejs.org/api/buffer.html). In order to sort string and binary keys the same way as other databases, `browser-level` internally converts data to a Uint8Array before passing them to IndexedDB. If you have no need to work with `Buffer` keys or values, you can choose to omit the [`buffer`](https://github.com/feross/buffer) shim from a JavaScript bundle (through configuration of Webpack, Browserify or other bundlers).

Due to limitations of IndexedDB, `browser-level` does not offer snapshot guarantees. Such a guarantee would mean that an iterator does not see the data of simultaneous writes - it would be reading from a snapshot in time. In contrast, a `browser-level` iterator reads a few entries ahead and then opens a new IndexedDB transaction on the next read. A "few" means all entries for `iterator.all()`, `size` amount of entries for `iterator.nextv(size)` and a hardcoded 100 entries for `iterator.next()`. Individual calls to those methods have snapshot guarantees but repeated calls do not.

The result is that an iterator will include the data of simultaneous writes, if `db.put()`, `db.del()` or `db.batch()` are called in between creating the iterator and consuming the iterator, or in between calls to `iterator.next()` or `iterator.nextv()`. For example:

```js
const iterator = db.iterator()
await db.put('abc', '123')

for await (const [key, value] of iterator) {
  // This might be 'abc'
  console.log(key)
}
```

If snapshot guarantees are a must for your application then use `iterator.all()` and call it immediately after creating the iterator:

```js
const entries = await db.iterator({ limit: 50 }).all()

// Synchronously iterate the result
for (const [key, value] of entries) {
  console.log(key)
}
```

### `db = new BrowserLevel(location[, options])`

Create a new database or open an existing one. The required `location` argument is the string name of the [`IDBDatabase`](https://developer.mozilla.org/en-US/docs/Web/API/IDBDatabase) to be opened, as well as the name of the object store within that database. The name of the `IDBDatabase` will be prefixed with `options.prefix`.

Besides `abstract-level` options, the optional `options` argument may contain:

- `prefix` (string, default: `'level-js-'`): Prefix for the `IDBDatabase` name. Can be set to an empty string. The default is compatible with `level-js`.
- `version` (string or number, default: `1`): The version to open the `IDBDatabase` with.

See [`IDBFactory#open()`](https://developer.mozilla.org/en-US/docs/Web/API/IDBFactory/open) for more details about database name and version.

### `BrowserLevel.destroy(location[, prefix][, callback])`

Delete the IndexedDB database at the given `location`. If `prefix` is not given, it defaults to the same value as the `BrowserLevel` constructor does. The `callback` function will be called when the destroy operation is complete, with a possible error argument. If no callback is provided, a promise is returned. This method is an additional method that is not part of the [`abstract-level`](https://github.com/Level/abstract-level) interface.

Before calling `destroy()`, close a database if it's using the same `location` and `prefix`:

```js
const db = new BrowserLevel('example')
await db.close()
await BrowserLevel.destroy('example')
```

## Install

With [npm](https://npmjs.org) do:

```bash
npm install browser-level
```

This module is best used with [`browserify`](http://browserify.org) or similar bundlers.

<!-- ## Big Thanks

Cross-browser Testing Platform and Open Source ♥ Provided by [Sauce Labs](https://saucelabs.com).

[![Sauce Labs logo](./sauce-labs.svg)](https://saucelabs.com) -->

## Contributing

[`Level/browser-level`](https://github.com/Level/browser-level) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

Support us with a monthly donation on [Open Collective](https://opencollective.com/level) and help us continue our work.

## License

[MIT](LICENSE)

[level-badge]: https://leveljs.org/img/badge.svg

[indexeddb]: https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API

[awesome]: https://github.com/Level/awesome

[abstract-level]: https://github.com/Level/abstract-level
