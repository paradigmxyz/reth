# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [`CHANGELOG`].

## v3

Dropped support for node 4. No other breaking changes.

## v2

This release drops Node.js 0.12, brings `memdown` up to par with latest [`levelup`] (v2) and [`abstract-leveldown`] (v4), simplifies serialization and removes global state.

### Targets latest [`levelup`]

Usage has changed to:

```js
const levelup = require('levelup')
const memdown = require('memdown')

const db = levelup(memdown())
```

From the old:

```js
const db = levelup('mydb', { db: memdown })
```

### No stringification of keys and values

This means that in addition to Buffers, you can store any JS type without the need for [`encoding-down`]. This release also makes behavior consistent in Node.js and browsers. Please refer to the [README](./README.md) for a detailed explanation.

### No global state or `location` argument

If you previously did this to make a global store:

```js
const db = levelup('mydb', { db: memdown })
```

You must now attach the store to a global yourself (if you desire global state):

```js
const db = window.mydb = levelup(memdown())
```

### No `null` batch operations

Instead of skipping `null` operations, `db.batch([null])` will throw an error courtesy of [`abstract-leveldown`].

[`CHANGELOG`]: CHANGELOG.md
[`abstract-leveldown`]: https://github.com/Level/abstract-leveldown
[`levelup`]: https://github.com/Level/levelup
[`encoding-down`]: https://github.com/Level/encoding-down
