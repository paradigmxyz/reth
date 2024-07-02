# memory-level

**In-memory [`abstract-level`][abstract-level] database for Node.js and browsers, backed by a [fully persistent red-black tree](https://www.npmjs.com/package/functional-red-black-tree).** The successor to [`memdown`](https://github.com/Level/memdown) and [`level-mem`](https://github.com/Level/mem).

> :pushpin: Which module should I use? What is `abstract-level`? Head over to the [FAQ](https://github.com/Level/community#faq).

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/memory-level.svg)](https://www.npmjs.com/package/memory-level)
[![Node version](https://img.shields.io/node/v/memory-level.svg)](https://www.npmjs.com/package/memory-level)
[![Test](https://img.shields.io/github/workflow/status/Level/memory-level/Test?label=test)](https://github.com/Level/memory-level/actions/workflows/test.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/memory-level?label=&logo=codecov&logoColor=fff)](https://codecov.io/gh/Level/memory-level)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective&logoColor=fff)](https://opencollective.com/level)

## Usage

_If you are upgrading: please see [`UPGRADING.md`](./UPGRADING.md)._

```js
const { MemoryLevel } = require('memory-level')

// Create a database
const db = new MemoryLevel({ valueEncoding: 'json' })

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

With callbacks:

```js
db.put('example', { hello: 'world' }, (err) => {
  if (err) throw err

  db.get('example', (err, value) => {
    if (err) throw err
    console.log(value) // { hello: 'world' }
  })
})
```

<!-- ## Browser support

[![Sauce Test Status](https://app.saucelabs.com/browser-matrix/level-ci.svg)](https://app.saucelabs.com/u/level-ci) -->

## API

The API of `memory-level` follows that of [`abstract-level`](https://github.com/Level/abstract-level) with a one additional constructor option (see below). The `createIfMissing` and `errorIfExists` options of `abstract-level` are not relevant here. Data is discarded when the last reference to the database is released (i.e. `db = null`). Closing or reopening the database has no effect on the data. Data is _not_ copied: when storing a Buffer value for example, subsequent mutations to that Buffer will affect the stored data too.

### `db = new MemoryLevel([options])`

Besides `abstract-level` options, the optional `options` object may contain:

- `storeEncoding` (string): one of `'buffer'`, `'view'`, `'utf8'`. How to store data internally. This affects which data types can be stored non-destructively. The default is `'buffer'` (that means Buffer) which is non-destructive. In browsers it may be preferable to use `'view'` (Uint8Array) to be able to exclude the [`buffer`](https://github.com/feross/buffer) shim. Or if there's no need to store binary data, then `'utf8'` (String). Regardless of the `storeEncoding`, `memory-level` supports input that is of any of the aforementioned types, but internally converts it to one type in order to provide a consistent sort order.

## Install

With [npm](https://npmjs.org) do:

```
npm install memory-level
```

## Contributing

[`Level/memory-level`](https://github.com/Level/memory-level) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

<!-- ## Big Thanks

Cross-browser Testing Platform and Open Source ♥ Provided by [Sauce Labs](https://saucelabs.com).

[![Sauce Labs logo](./sauce-labs.svg)](https://saucelabs.com) -->

## License

[MIT](LICENSE)

[abstract-level]: https://github.com/Level/abstract-level

[level-badge]: https://leveljs.org/img/badge.svg
