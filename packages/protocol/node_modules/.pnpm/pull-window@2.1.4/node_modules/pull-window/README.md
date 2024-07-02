# pull-window

Aggregate a pull-stream into windows.

Several helpers are provided for particular types of windows,
sliding, tumbling, etc.

And also, a low level 

## Example: "tumbling" window

sum every 10 items.

``` js
var pull   = require('pull-stream')
var window = require('pull-window')

function everyTen () {
  var i = 0
  //window calls init with each data item,
  //and a callback to close that window.
  return window(function (data, cb) {
    //if you don't want to start a window here,
    //return undefined
    if(i != 0) return
    var sum = 0

    //else return a function.
    //this will be called all data
    //until you callback.
    return function (end, data) {
      if(end) return cb(null, sum)
      sum += data
      if(++i >= 10) {
        i = 0
        cb(null, sum)
      }
    }
  }
}

pull(
  pull.count(1000),
  everyTen(),
  pull.log()
)
```

## Example: variable sized window

Each window doesn't have to be the same size...

``` js
var pull   = require('pull-stream')
var window = require('pull-window')

function groupTo100 () {
  var sum = null
  return window(function (_, cb) {
    if(sum != null) return

    //sum stuff together until you have 100 or more
    return function (end, data) {
      if(end) return cb(null, sum)
      sum += data
      if(sum >= 100) {
        //copy sum like this, incase the next item
        //comes through sync
        var _sum = sum; sum = null
        cb(null, _sum)
      }
    }
  })
}

pull(
  pull.count(1000)
  groupTo100(),
  pull.log()
)
```

## Example: sliding window

to make more over lapping windows
just return the window function more often.

``` js
var pull   = require('pull-stream')
var window = require('pull-window')

function sliding () {
  return window(function (_, cb) {
    var sum = 0, i = 0

    //sum stuff together until you have 100 or more
    return function (end, data) {
      if(end) return cb(null, sum)
      sum += data
      if(++i >= 10) {
        //in this example, each window gets it's own sum,
        //so we don't need to copy it.
        cb(null, sum)
      }
    }
  })
}

pull(
  pull.count(100)
  sliding(),
  pull.log()
)
```


## API


### window (start, map)
``` js

window(function startWindow (data, cb) {

  //called on each chunk
  //including the first one
  return function addToWindow (end, data) {
    //cb(null, aggregate) when done.
  }
}, function mapWindow (start, data) {
  //(optional)
  //map the window to something that tracks start, also
})
```

By default, windows are mapped to `{start: firstData, data: aggregate}`.
unless you pass in an different `mapWindow` function.


### window.sliding(reduce, size)

reduce every `size` items into a single value, in a sliding window

### window.recent(size, time)

tumbling window that groups items onto an array,
either every `size` items, or within `time` ms,
which ever occurs earliest. 

## License

MIT
