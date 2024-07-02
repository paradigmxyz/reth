# is-fn [![Build Status](https://travis-ci.org/sindresorhus/is-fn.svg?branch=master)](https://travis-ci.org/sindresorhus/is-fn)

> Check if a value is a function


## Install

```
$ npm install --save is-fn
```


## Usage

```js
const isFn = require('is-fn');

isFn(function () {});
//=> true

isFn('unicorn');
//=> false
```


## License

MIT © [Sindre Sorhus](http://sindresorhus.com)
