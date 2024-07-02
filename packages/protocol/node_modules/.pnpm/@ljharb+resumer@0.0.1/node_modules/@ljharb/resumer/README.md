# @ljharb/resumer <sup>[![Version Badge][npm-version-svg]][package-url]</sup>

*Note*: This package is a fork of https://npmjs.com/resumer, and builds off of it.

Return a through stream that starts out paused and resumes on the next tick, unless somebody called `.pause()`.

This module has the same signature as [through](https://npmjs.com/package/through).

[![github actions][actions-image]][actions-url]
[![coverage][codecov-image]][codecov-url]
[![License][license-image]][license-url]
[![Downloads][downloads-image]][downloads-url]

[![npm badge][npm-badge-png]][package-url]

# example

``` js
var resumer = require('resumer');
var s = createStream();
s.pipe(process.stdout);

function createStream () {
    var stream = resumer();
    stream.queue('beep boop\n');
    return stream;
}
```

```
$ node example/resume.js
beep boop
```

# methods

``` js
var resumer = require('@ljharb/resumer')
```

## resumer(write, end)

Return a new through stream from `write` and `end`, which default to pass-through `.queue()` functions if not specified.

The stream starts out paused and will be resumed on the next tick unless you call `.pause()` first.

`write` and `end` get passed directly through to [through](https://npmjs.com/package/through).

# install

With [npm](https://npmjs.org) do:

```
npm install @ljharb/resumer
```

# license

MIT

[package-url]: https://npmjs.org/package/@ljharb/resumer
[npm-version-svg]: https://versionbadg.es/ljharb/resumer.svg
[deps-svg]: https://david-dm.org/ljharb/resumer.svg
[deps-url]: https://david-dm.org/ljharb/resumer
[dev-deps-svg]: https://david-dm.org/ljharb/resumer/dev-status.svg
[dev-deps-url]: https://david-dm.org/ljharb/resumer#info=devDependencies
[npm-badge-png]: https://nodei.co/npm/@ljharb/resumer.png?downloads=true&stars=true
[license-image]: https://img.shields.io/npm/l/@ljharb/resumer.svg
[license-url]: LICENSE
[downloads-image]: https://img.shields.io/npm/dm/@ljharb/resumer.svg
[downloads-url]: https://npm-stat.com/charts.html?package=@ljharb/resumer
[codecov-image]: https://codecov.io/gh/ljharb/resumer/branch/main/graphs/badge.svg
[codecov-url]: https://app.codecov.io/gh/ljharb/resumer/
[actions-image]: https://img.shields.io/endpoint?url=https://github-actions-badge-u3jn4tfpocch.runkit.sh/ljharb/resumer
[actions-url]: https://github.com/ljharb/resumer/actions
