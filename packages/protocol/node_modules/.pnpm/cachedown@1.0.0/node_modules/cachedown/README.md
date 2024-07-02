# cachedown

leveldown with a cache, for fast puts and gets

[![Build Status](https://travis-ci.org/mvayngrib/cachedown.png)](https://travis-ci.org/mvayngrib/cachedown)

## Usage

```js
var slow = require('leveldown')
var fast = require('cachedown')
var levelup = require('levelup')

var db = levelup('./db.db', {
  db: function (location) {
    // careful! this db has a max cache size of Infinity!
    // to limit cache size, use fast(location, slow).maxSize(/* max size */)
    return fast(location, slow)
  }
})

// use db with better performance for puts and gets
db.put('hey', 'ho', function (err) {
  db.get('hey', function (err, val) {
    // val comes from internal cache
    console.log(val) // ho
  })
})
```

### Misc

```js
// 1

var leveldown = require('leveldown')
var levelup = require('levelup')
var cachedown = require('cachedown')
// set default leveldown
cachedown.setLeveldown(leveldown)
var db = levelup('path/to/db', { db: cachedown })

// 2

var cachedownInstance = new cachedown('path/to/db')
// change max size
cachedownInstance.maxSize(100)
// clear cache
cachedownInstance.clearCache()
```
