# memdown

> In-memory [`abstract-leveldown`] store for Node.js and browsers.

[![level badge][level-badge]](https://github.com/Level/awesome)
[![npm](https://img.shields.io/npm/v/memdown.svg?label=&logo=npm)](https://www.npmjs.com/package/memdown)
[![Node version](https://img.shields.io/node/v/memdown.svg)](https://www.npmjs.com/package/memdown)
[![Travis](https://img.shields.io/travis/Level/memdown.svg?logo=travis&label=)](https://travis-ci.org/Level/memdown)
[![Coverage Status](https://coveralls.io/repos/Level/memdown/badge.svg?branch=master&service=github)](https://coveralls.io/github/Level/memdown?branch=master)
[![JavaScript Style Guide](https://img.shields.io/badge/code_style-standard-brightgreen.svg)](https://standardjs.com)
[![npm](https://img.shields.io/npm/dm/memdown.svg?label=dl)](https://www.npmjs.com/package/memdown)
[![Backers on Open Collective](https://opencollective.com/level/backers/badge.svg?color=orange)](#backers)
[![Sponsors on Open Collective](https://opencollective.com/level/sponsors/badge.svg?color=orange)](#sponsors)

## Example

**If you are upgrading:** please see the [upgrade guide](./UPGRADING.md).

```js
const levelup = require('levelup')
const memdown = require('memdown')

const db = levelup(memdown())

db.put('hey', 'you', (err) => {
  if (err) throw err

  db.get('hey', { asBuffer: false }, (err, value) => {
    if (err) throw err
    console.log(value) // 'you'
  })
})
```

Your data is discarded when the process ends or you release a reference to the store. Note as well, though the internals of `memdown` operate synchronously - [`levelup`] does not.

## Browser support

[![Sauce Test Status](https://saucelabs.com/browser-matrix/level-ci.svg)](https://saucelabs.com/u/level-ci)

## Data types

Keys and values can be strings or Buffers. Any other key type will be irreversibly stringified. The only exceptions are `null` and `undefined`. Keys and values of that type are rejected.

```js
const db = levelup(memdown())

db.put('example', 123, (err) => {
  if (err) throw err

  db.createReadStream({
    keyAsBuffer: false,
    valueAsBuffer: false
  }).on('data', (entry) => {
    console.log(typeof entry.key) // 'string'
    console.log(typeof entry.value) // 'string'
  })
})
```

If you desire non-destructive encoding (e.g. to store and retrieve numbers as-is), wrap `memdown` with [`encoding-down`]. Alternatively install [`level-mem`] which conveniently bundles [`levelup`], `memdown` and [`encoding-down`]. Such an approach is also recommended if you want to achieve universal (isomorphic) behavior. For example, you could have [`leveldown`] in a backend and `memdown` in the frontend.

```js
const encode = require('encoding-down')
const db = levelup(encode(memdown(), { valueEncoding: 'json' }))

db.put('example', 123, (err) => {
  if (err) throw err

  db.createReadStream({
    keyAsBuffer: false,
    valueAsBuffer: false
  }).on('data', (entry) => {
    console.log(typeof entry.key) // 'string'
    console.log(typeof entry.value) // 'number'
  })
})
```

## Snapshot guarantees

A `memdown` store is backed by [a fully persistent data structure](https://www.npmjs.com/package/functional-red-black-tree) and thus has snapshot guarantees. Meaning that reads operate on a snapshot in time, unaffected by simultaneous writes.

## Test

In addition to the regular `npm test`, you can test `memdown` in a browser of choice with:

```
npm run test-browser-local
```

To check code coverage:

```
npm run coverage
```

## Contributing

[`Level/memdown`](https://github.com/Level/memdown) is an **OPEN Open Source Project**. This means that:

> Individuals making significant and valuable contributions are given commit-access to the project to contribute as they see fit. This project is more like an open wiki than a standard guarded open source project.

See the [Contribution Guide](https://github.com/Level/community/blob/master/CONTRIBUTING.md) for more details.

## Big Thanks

Cross-browser Testing Platform and Open Source ♥ Provided by [Sauce Labs](https://saucelabs.com).

[![Sauce Labs logo](./sauce-labs.svg)](https://saucelabs.com)

## Donate

To sustain [`Level`](https://github.com/Level) and its activities, become a backer or sponsor on [Open Collective](https://opencollective.com/level). Your logo or avatar will be displayed on our 28+ [GitHub repositories](https://github.com/Level) and [npm](https://www.npmjs.com/) packages. 💖

### Backers

[![Open Collective backers](https://opencollective.com/level/backers.svg?width=890)](https://opencollective.com/level)

### Sponsors

[![Open Collective sponsors](https://opencollective.com/level/sponsors.svg?width=890)](https://opencollective.com/level)

## License

[MIT](LICENSE.md) © 2013-present Rod Vagg and [Contributors](CONTRIBUTORS.md).

[`abstract-leveldown`]: https://github.com/Level/abstract-leveldown

[`levelup`]: https://github.com/Level/levelup

[`encoding-down`]: https://github.com/Level/encoding-down

[`leveldown`]: https://github.com/Level/leveldown

[`level-mem`]: https://github.com/Level/mem

[level-badge]: https://leveljs.org/img/badge.svg
