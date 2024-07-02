# level-supports

**Create a manifest describing the abilities of an [`abstract-level`](https://github.com/Level/abstract-level) database.** No longer compatible with [`levelup`](https://github.com/Level/levelup) or [`abstract-leveldown`](https://github.com/Level/abstract-leveldown) since version 3.0.0.

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/level-supports.svg)](https://www.npmjs.com/package/level-supports)
[![Node version](https://img.shields.io/node/v/level-supports.svg)](https://www.npmjs.com/package/level-supports)
[![Test](https://img.shields.io/github/workflow/status/Level/supports/Test?label=test)](https://github.com/Level/supports/actions/workflows/test.yml)
[![Coverage](https://img.shields.io/codecov/c/github/Level/supports?label=\&logo=codecov\&logoColor=fff)](https://codecov.io/gh/Level/supports)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript\&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)
[![Donate](https://img.shields.io/badge/donate-orange?logo=open-collective\&logoColor=fff)](https://opencollective.com/level)

## Usage

```js
const { supports } = require('level-supports')

db.supports = supports({
  permanence: false,
  encodings: {
    utf8: true
  }
})
```

Receivers of the db can then use it like so:

```js
if (!db.supports.permanence) {
  throw new Error('Persistent storage is required')
}
```

## API

### `manifest = supports([manifest, ..])`

Given zero or more manifest objects, returns a merged and enriched manifest object that has truthy properties for each of the features listed below.

For future extensibility, the properties are truthy rather than strictly typed booleans. Falsy or absent properties are converted to `false`, other values are allowed:

```js
supports().snapshots // false
supports({ snapshots: true }).snapshots // true
supports({ snapshots: {} }).snapshots // {}
supports({ snapshots: 1 }, { snapshots: 2 }).snapshots // 2
```

For consumers of the manifest this means they should check support like so:

```js
if (db.supports.snapshots)
```

Rather than:

```js
if (db.supports.snapshots === true)
```

**Note:** the manifest describes high-level features that typically encompass multiple methods of a db. It is currently not a goal to describe a full API, or versions of it.

## Features

### `snapshots` (boolean)

Does the database have snapshot guarantees? Meaning that reads are unaffected by simultaneous writes. For example, an iterator should read from a snapshot of the database, created at the time `db.iterator()` was called. This means the iterator will not see the data of simultaneous write operations.

Must be `false` if any of the following is true:

- Reads don't operate on a snapshot
- Snapshots are created asynchronously.

<details>
<summary>Support matrix</summary>

| Module               | Snapshot guarantee          |
| :------------------- | :-------------------------- |
| `classic-level`      | ✅                           |
| `memory-level`       | ✅                           |
| `browser-level`      | ❌                           |
| `rocks-level`        | ✅                           |
| `leveldown`          | ✅                           |
| `rocksdb`            | ✅                           |
| `memdown`            | ✅                           |
| `level-js`           | ✅ (by buffering)            |
| `encoding-down`      | ✅                           |
| `deferred-leveldown` | ✅                           |
| `levelup`            | ✅                           |
| `level-packager`     | ✅                           |
| `level`              | ✅                           |
| `level-mem`          | ✅                           |
| `level-rocksdb`      | ✅                           |
| `subleveldown`       | ✅                           |
| `multileveldown`     | ✅ (unless `retry` is true)  |
| `level-party`        | ❌ (unless `retry` is false) |

</details>

### `permanence` (boolean)

Does data survive after process (or environment) exit? Typically true. False for [`memory-level`](https://github.com/Level/memory-level) and [`memdown`](https://github.com/Level/memdown).

### `seek` (boolean)

Do iterators support [`seek(..)`](https://github.com/Level/abstract-level/#iteratorseektarget-options)?

<details>
<summary>Support matrix</summary>

| Module               | Support |
| :------------------- | :------ |
| `abstract-level`     | ✅ 1.0.0 |
| `classic-level`      | ✅ 1.0.0 |
| `memory-level`       | ✅ 1.0.0 |
| `browser-level`      | ✅ 1.0.0 |
| `rocks-level`        | ✅ 1.0.0 |
| `abstract-leveldown` | ✅ 6.0.0 |
| `leveldown`          | ✅ 1.2.0 |
| `rocksdb`            | ✅ 1.0.0 |
| `memdown`            | ✅ 4.1.0 |
| `level-js`           | ❌       |
| `encoding-down`      | ✅ 6.1.0 |
| `deferred-leveldown` | ✅ 5.1.0 |
| `levelup`            | ✅ n/a   |
| `level-packager`     | ✅ n/a   |
| `level`              | ✅ 8.0.0 |
| `level-mem`          | ✅ 4.0.0 |
| `level-rocksdb`      | ✅ 1.0.0 |
| `subleveldown`       | ✅ 4.1.0 |
| `multileveldown`     | ❌       |
| `level-party`        | ❌       |

</details>

#### `clear` (boolean)

Does the database support `db.clear()`? Always true since `abstract-level@1`.

<details>
<summary>Support matrix</summary>

See also [Level/community#79](https://github.com/Level/community/issues/79).

| Module                          | Support | Optimized |
| :------------------------------ | :------ | :-------- |
| `abstract-level` and dependents | ✅ 1.0.0 | ✅ 1.0.0   |
| `abstract-leveldown`            | ✅ 6.1.0 | n/a       |
| `leveldown`                     | ✅ 5.2.0 | ✅ 6.0.3   |
| `rocksdb`                       | ✅ 4.1.0 | ✅ 5.2.0   |
| `memdown`                       | ✅ 5.0.0 | ✅ 6.1.1   |
| `level-js`                      | ✅ 5.0.0 | ✅ 5.0.0   |
| `encoding-down`                 | ✅ 6.2.0 | n/a       |
| `deferred-leveldown`            | ✅ 5.2.0 | n/a       |
| `levelup`                       | ✅ 4.2.0 | n/a       |
| `level-packager`                | ✅ 5.0.3 | n/a       |
| `level`                         | ✅ 6.0.0 | ✅ 7.0.1   |
| `level-mem`                     | ✅ 5.0.1 | ✅ 6.0.1   |
| `level-rocksdb`                 | ✅ 5.0.0 | ✅ 5.0.0   |
| `subleveldown`                  | ✅ 4.2.1 | ✅ 4.2.1   |
| `multileveldown`                | ✅ 5.0.0 | ✅ 5.0.0   |
| `level-party`                   | ✅ 5.1.0 | ✅ 5.1.0   |

</details>

### `status` (boolean)

Does the database have a [`status`](https://github.com/Level/abstract-level/#dbstatus) property? Always true since `abstract-level@1`.

### `deferredOpen` (boolean)

Can operations like `db.put()` be called without explicitly opening the db? Like so:

```js
const db = new Level()
await db.put('key', 'value')
```

Always true since `abstract-level@1`.

### `createIfMissing`, `errorIfExists` (boolean)

Does `db.open()` support these options?

<details>
<summary>Support matrix</summary>

| Module          | Support |
| :-------------- | :------ |
| `classic-level` | ✅       |
| `rocks-level`   | ✅       |
| `memory-level`  | ❌       |
| `browser-level` | ❌       |
| `leveldown`     | ✅       |
| `rocksdb`       | ✅       |
| `memdown`       | ❌       |
| `level-js`      | ❌       |

</details>

### `promises` (boolean)

Do all database methods (that don't otherwise have a return value) support promises, in addition to callbacks? Such that, when a callback argument is omitted, a promise is returned:

```js
db.put('key', 'value', callback)
await db.put('key', 'value')
```

Always true since `abstract-level@1`.

<details>
<summary>Support matrix</summary>

| Module                              | Support              |
| :---------------------------------- | :------------------- |
| `abstract-level` and dependents     | ✅                    |
| `abstract-leveldown` and dependents | ❌ (except iterators) |
| `levelup`                           | ✅                    |
| `level-packager`                    | ✅                    |
| `level`                             | ✅                    |
| `level-mem`                         | ✅                    |
| `level-rocksdb`                     | ✅                    |
| `subleveldown`                      | ✅                    |
| `multileveldown`                    | ✅                    |
| `level-party`                       | ✅                    |

</details>

### `events` (object)

Which events does the database emit, as indicated by nested properties? For example:

```js
if (db.supports.events.put) {
  db.on('put', listener)
}
```

### `streams` (boolean)

Does database have the methods `createReadStream`, `createKeyStream` and `createValueStream`, following the API documented in `levelup`? For `abstract-level` databases, a standalone module called [`level-read-stream`](https://github.com/Level/read-stream) is available.

<details>
<summary>Support matrix</summary>

| Module                              | Support |
| :---------------------------------- | :------ |
| `abstract-level` and dependents     | ❌       |
| `abstract-leveldown` and dependents | ❌       |
| `levelup`                           | ✅       |
| `level-packager`                    | ✅       |
| `level`                             | ✅       |
| `level-mem`                         | ✅       |
| `level-rocksdb`                     | ✅       |
| `subleveldown`                      | ✅       |
| `multileveldown`                    | ✅       |
| `level-party`                       | ✅       |

</details>

### `encodings` (object)

Which encodings (by name) does the database support, as indicated by nested properties? For example:

```js
{ utf8: true, json: true }
```

As the `encodings` property cannot be false (anymore, since `level-supports` v3.0.0) it implies that the database supports `keyEncoding` and `valueEncoding` options on all relevant methods, uses a default encoding of utf8 and that hence all of its read operations return strings rather than buffers by default.

<details>
<summary>Support matrix (general support)</summary>

_This matrix just indicates general support of encodings as a feature, not that the listed modules support the `encodings` property exactly as described above, which only works on `abstract-level` databases._

| Module                                 | Support |
| :------------------------------------- | :------ |
| `abstract-level` (and dependents)      | ✅       |
| `abstract-leveldown`  (and dependents) | ❌       |
| `encoding-down`                        | ✅       |
| `levelup`                              | ✅       |
| `level-packager`                       | ✅       |
| `level`                                | ✅       |
| `level-mem`                            | ✅       |
| `level-rocksdb`                        | ✅       |
| `subleveldown`                         | ✅       |
| `multileveldown`                       | ✅       |
| `level-party`                          | ✅       |

</details>

<details>
<summary>Support matrix (specific encodings)</summary>

_This matrix lists which encodings are supported as indicated by e.g. `db.supports.encodings.utf8`. Encodings that encode to another (like how `'json'` encodes to `'utf8'`) are excluded here, though they are present in `db.supports.encodings`._

| Module          | `'utf8'`      | `'buffer'`    | `'view'`      |
| :-------------- | :------------ | :------------ | :------------ |
| `classic-level` | ✅             | ✅             | ✅ <sup>1<sup> |
| `memory-level`  | ✅ <sup>2<sup> | ✅ <sup>2<sup> | ✅ <sup>2<sup> |
| `browser-level` | ✅ <sup>1<sup> | ✅ <sup>1<sup> | ✅             |
| `rocks-level`   | ✅             | ✅             | ✅ <sup>1<sup> |
| `level@8`       | ✅ <sup>3<sup> | ✅ <sup>3<sup> | ✅ <sup>3<sup> |

<small>

1. Transcoded (which may have a performance impact).
2. Can be controlled via `storeEncoding` option.
3. Whether it's transcoded depends on environment (Node.js or browser).

</small>

</details>

### `getMany` (boolean)

Does the database support `db.getMany()`? Always true since `abstract-level@1`.

<details>
<summary>Support matrix</summary>

| Module                          | Support |
| :------------------------------ | :------ |
| `abstract-level` and dependents | ✅ 1.0.0 |
| `abstract-leveldown`            | ✅ 7.2.0 |
| `leveldown`                     | ✅ 6.1.0 |
| `rocksdb`                       | ✅ 5.2.0 |
| `memdown`                       | ✅       |
| `level-js`                      | ✅ 6.1.0 |
| `encoding-down`                 | ✅ 7.1.0 |
| `deferred-leveldown`            | ✅ 7.0.0 |
| `levelup`                       | ✅ 5.1.0 |
| `level`                         | ✅ 7.0.1 |
| `level-mem`                     | ✅ 6.0.1 |
| `level-rocksdb`                 | ✅ 5.0.0 |
| `subleveldown`                  | ✅ 6.0.0 |
| `multileveldown`                | ✅ 5.0.0 |
| `level-party`                   | ✅ 5.1.0 |

</details>

### `keyIterator` (boolean)

Does the database have a `keys([options])` method that returns a key iterator? Always true since `abstract-level@1`.

### `valueIterator` (boolean)

Does the database have a `values([options])` method that returns a key iterator? Always true since `abstract-level@1`.

### `iteratorNextv` (boolean)

Do iterators have a `nextv(size[, options][, callback])` method? Always true since `abstract-level@1`.

### `iteratorAll` (boolean)

Do iterators have a `all([options][, callback])` method? Always true since `abstract-level@1`.

### `additionalMethods` (object)

Declares support of additional methods, that are not part of the `abstract-level` interface. In the form of:

```js
{
  foo: true,
  bar: true
}
```

Which says the db has two methods, `foo` and `bar`. It might be used like so:

```js
if (db.supports.additionalMethods.foo) {
  db.foo()
}
```

For future extensibility, the properties of `additionalMethods` should be taken as truthy rather than strictly typed booleans. We may add additional metadata (see [#1](https://github.com/Level/supports/issues/1)).

## Install

With [npm](https://npmjs.org) do:

```
npm install level-supports
```

## Contributing

[`Level/supports`](https://github.com/Level/supports) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

Support us with a monthly donation on [Open Collective](https://opencollective.com/level) and help us continue our work.

## License

[MIT](LICENSE)

[level-badge]: https://leveljs.org/img/badge.svg
