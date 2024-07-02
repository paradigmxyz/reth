# looper

Loop with callbacks but don't RangeError

[![travis](https://travis-ci.org/dominictarr/looper.png?branch=master)
](https://travis-ci.org/dominictarr/looper)

[![testling](http://ci.testling.com/dominictarr/looper.png)
](http://ci.testling.com/dominictarr/looper)

## Synopsis

Normally, if `mightBeAsync` calls it's cb immediately
this would `RangeError`:

``` js
var l = 100000
;(function next () {
  if(--l) mightBeAsync(next)
})
```

`looper` detects that case, and falls back to a `while` loop,

## Example

``` js
var loop = require('looper')

var l = 100000
loop(function () {
  var next = this
  if(--l) probablySync(next)
})()
```

when you want to stop looping, don't call `next`.
`looper` checks if each callback is sync or not,
so you can even mix sync and async calls!

## License

MIT
