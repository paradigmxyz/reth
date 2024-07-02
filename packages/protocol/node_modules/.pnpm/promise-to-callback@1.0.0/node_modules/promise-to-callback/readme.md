# promise-to-callback [![Build Status](https://travis-ci.org/stevemao/promise-to-callback.svg?branch=master)](https://travis-ci.org/stevemao/promise-to-callback)

> Convert promise to callback interface

Because there are many promise implementations and callbacks are better to handle errors.


## Install

```
$ npm install --save promise-to-callback
```


## Usage

```js
var promiseToCallback = require('promise-to-callback');

promiseToCallback(promise)(function(err, data) {
	...
});
```


## License

MIT © [Steve Mao](https://github.com/stevemao)
