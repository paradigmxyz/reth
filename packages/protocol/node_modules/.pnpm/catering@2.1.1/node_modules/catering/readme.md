# catering

**Cater to callback and promise crowds.**  
Simple utility to allow your module to be consumed with a callback or promise. For Node.js and browsers.

[![npm status](https://img.shields.io/npm/v/catering.svg)](https://www.npmjs.com/package/catering)
[![Node version](https://img.shields.io/node/v/catering.svg)](https://www.npmjs.com/package/catering)
[![Test](https://img.shields.io/github/workflow/status/vweevers/catering/Test?label=test)](https://github.com/vweevers/catering/actions/workflows/test.yml)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript&logoColor=fff)](https://standardjs.com)

## Menu

If your module internally uses callbacks:

```js
const { fromCallback } = require('catering')
const kPromise = Symbol('promise')

module.exports = function (callback) {
  callback = fromCallback(callback, kPromise)
  queueMicrotask(() => callback(null, 'example'))
  return callback[kPromise]
}
```

If your module internally uses promises:

```js
const { fromPromise } = require('catering')

module.exports = function (callback) {
  return fromPromise(Promise.resolve('example'), callback)
}
```

Either way your module can now be consumed in two ways:

```js
example((err, result) => {})
const result = await example()
```

When converting from a promise to a callback, `fromPromise` calls the callback in a next tick to escape the promise chain and not let it steal your beautiful errors.

## Install

With [npm](https://npmjs.org) do:

```
npm install catering
```

## License

[MIT](LICENSE) © 2018-present Vincent Weevers. Originally extracted from [`levelup`](https://github.com/Level/levelup/blob/37e0270c8c29d5086904e29e247e918dddcce6e2/lib/promisify.js).
