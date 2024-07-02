# run-parallel-limit [![travis][travis-image]][travis-url] [![npm][npm-image]][npm-url] [![downloads][downloads-image]][downloads-url] [![javascript style guide][standard-image]][standard-url]

[travis-image]: https://img.shields.io/travis/feross/run-parallel-limit/master.svg
[travis-url]: https://travis-ci.org/feross/run-parallel-limit
[npm-image]: https://img.shields.io/npm/v/run-parallel-limit.svg
[npm-url]: https://npmjs.org/package/run-parallel-limit
[downloads-image]: https://img.shields.io/npm/dm/run-parallel-limit.svg
[downloads-url]: https://npmjs.org/package/run-parallel-limit
[standard-image]: https://img.shields.io/badge/code_style-standard-brightgreen.svg
[standard-url]: https://standardjs.com

### Run an array of functions in parallel, but limit the number of tasks executing at the same time

![run-parallel-limit](img.png) [![Sauce Test Status](https://saucelabs.com/browser-matrix/run-parallel-limit.svg)](https://saucelabs.com/u/run-parallel-limit)

### install

```
npm install run-parallel-limit
```

### usage

#### parallelLimit(tasks, limit, [callback])

Run the `tasks` array of functions in parallel, with a maximum of `limit` tasks executing
at the same time. If any of the functions pass an error to its callback, the main
`callback` is immediately called with the value of the error. Once the `tasks` have
completed, the results are passed to the final `callback` as an array.

Note that the `tasks` are not executed in batches, so there is no guarantee that the first
`limit` tasks will complete before any others are started.

It is also possible to use an object instead of an array. Each property will be run as a
function and the results will be passed to the final `callback` as an object instead of
an array. This can be a more readable way of handling the results.

##### arguments

- `tasks` - An array or object containing functions to run. Each function is passed a
`callback(err, result)` which it must call on completion with an error `err` (which can
be `null`) and an optional `result` value.
- `limit` - The maximum number of `tasks` to run at any time.
- `callback(err, results)` - An optional callback to run once all the functions have
completed. This function gets a results array (or object) containing all the result
arguments passed to the task callbacks.

##### example

```js
var parallelLimit = require('run-parallel-limit')

var tasks = [
  function (callback) {
    setTimeout(function () {
      callback(null, 'one')
    }, 200)
  },
  function (callback) {
    setTimeout(function () {
      callback(null, 'two')
    }, 100)
  },
  ... hundreds more tasks ...
]

parallelLimit(tasks, 5, function (err, results) {
  // optional callback
  // the results array will equal ['one', 'two', ...] even though
  // the second function had a shorter timeout.
})
```

The above code runs with a concurrency `limit` of 5, so at most 5 tasks will be running at
any given time.

This module is basically equavalent to
[`async.parallelLimit`](https://github.com/caolan/async#parallellimittasks-limit-callback),
but it's handy to just have the one function you need instead of the kitchen sink.
Modularity! Especially handy if you're serving to the browser and need to reduce your
javascript bundle size.

Works great in the browser with [browserify](http://browserify.org/)!

### see also

- [run-auto](https://github.com/feross/run-auto)
- [run-parallel](https://github.com/feross/run-parallel)
- [run-series](https://github.com/feross/run-series)
- [run-waterfall](https://github.com/feross/run-waterfall)

### license

MIT. Copyright (c) [Feross Aboukhadijeh](http://feross.org).
