# level-post

get consistent post hooks for leveldb.

[![travis](https://travis-ci.org/dominictarr/level-post.png?branch=master)
](https://travis-ci.org/dominictarr/level-post)

[![testling](http://ci.testling.com/dominictarr/level-post.png)
](http://ci.testling.com/dominictarr/level-post)

``` js
var level = require('level')

var db = level('/tmp/whatever-db')

post(db, function (op) {
  //this is called after every put, del, or batch
  console.log(op)
})

db.put('foo', 'bar', function (err) {
  //...
})
```

# methods

## post(db, opts={}, cb)

Create a hook to listen for database events matching the constraints in `opts`.
`cb(op)` fires for each matching operation for `op`, an object with `type`,
`key`, and `value` properties.

You can use these keys as constraints, just like level core:

* `opts.gte`, `opts.start`, `opts.min` - greater than or equal to
* `opts.gt` - greater than
* `opts.lte`, `opts.end`, `opts.max` - less than or equal to
* `opts.lt` - less than

You can also specify a keyEncoding with `opts.keyEncoding`. If there was a
keyEncoding set up by leveldb in the constructor (at `db.options.keyEncoding`),
that one will be used.

## License

MIT
