# level-mem

> A convenience package that bundles [`levelup`](https://github.com/Level/levelup) and [`memdown`](https://github.com/Level/memdown) and exposes `levelup` on its export.

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/level-mem.svg?label=&logo=npm)](https://www.npmjs.com/package/level-mem)
[![Node version](https://img.shields.io/node/v/level-mem.svg)](https://www.npmjs.com/package/level-mem)
[![Travis](https://img.shields.io/travis/Level/mem.svg?logo=travis&label=)](https://travis-ci.org/Level/mem)
[![Coverage Status](https://coveralls.io/repos/github/Level/mem/badge.svg)](https://coveralls.io/github/Level/mem)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)
[![npm](https://img.shields.io/npm/dm/level-mem.svg?label=dl)](https://www.npmjs.com/package/level-mem)
[![Backers on Open Collective](https://opencollective.com/level/backers/badge.svg?color=orange)](#backers)
[![Sponsors on Open Collective](https://opencollective.com/level/sponsors/badge.svg?color=orange)](#sponsors)

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

See [`levelup`](https://github.com/Level/levelup) and [`memdown`](https://github.com/Level/memdown) for more details.

**If you are upgrading:** please see [`UPGRADING.md`](UPGRADING.md).

## Contributing

[`Level/mem`](https://github.com/Level/mem) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Donate

To sustain [`Level`](https://github.com/Level) and its activities, become a backer or sponsor on [Open Collective](https://opencollective.com/level). Your logo or avatar will be displayed on our 28+ [GitHub repositories](https://github.com/Level) and [npm](https://www.npmjs.com/) packages. 💖

### Backers

[![Open Collective backers](https://opencollective.com/level/backers.svg?width=890)](https://opencollective.com/level)

### Sponsors

[![Open Collective sponsors](https://opencollective.com/level/sponsors.svg?width=890)](https://opencollective.com/level)

## License

[MIT](LICENSE.md) © 2012-present [Contributors](CONTRIBUTORS.md).

[level-badge]: https://leveljs.org/img/badge.svg
