# level-mem

> A convenience package that bundles [`levelup`](https://github.com/level/levelup) and [`memdown`](https://github.com/level/memdown) and exposes `levelup` on its export.

[![level badge][level-badge]](https://github.com/level/awesome)
[![npm](https://img.shields.io/npm/v/level-mem.svg)](https://www.npmjs.com/package/level-mem)
![Node version](https://img.shields.io/node/v/level-mem.svg)
[![Build Status](https://secure.travis-ci.org/Level/mem.png)](http://travis-ci.org/Level/mem)
[![david](https://david-dm.org/Level/mem.svg)](https://david-dm.org/level/mem)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)
[![npm](https://img.shields.io/npm/dm/level-mem.svg)](https://www.npmjs.com/package/level-mem)

Use this package to avoid having to explicitly install `memdown` when you want to use `memdown` with `levelup` for non-persistent `levelup` data storage.

```js
const level = require('level-mem')

// 1) Create our database, with optional options.
//    This will create or open the underlying LevelDB store.
const db = level()

// 2) Put a key & value
db.put('name', 'Level', function (err) {
  if (err) return console.log('Ooops!', err) // some kind of I/O error

  // 3) Fetch by key
  db.get('name', function (err, value) {
    if (err) return console.log('Ooops!', err) // likely the key was not found

    // Ta da!
    console.log('name=' + value)
  })
})
```

See [`levelup`](https://github.com/level/levelup) and [`memdown`](https://github.com/level/memdown) for more details.

**If you are upgrading:** please see [`UPGRADING.md`](UPGRADING.md).

## Contributing

`level-mem` is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [CONTRIBUTING.md](https://github.com/Level/level/blob/master/CONTRIBUTING.md) file for more details.

## License

Copyright (c) 2012-present `level-mem` [contributors](https://github.com/level/community#contributors).

`level-mem` is licensed under the MIT license. All rights not explicitly granted in the MIT license are reserved. See the included [`LICENSE.md`](LICENSE.md) file for more details.

[level-badge]: http://leveldb.org/img/badge.svg
