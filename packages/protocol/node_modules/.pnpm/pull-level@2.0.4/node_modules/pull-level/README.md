# pull-level

[pull-stream](https://github.com/dominictarr/pull-stream) interface to
[levelup](https://github.com/rvagg/node-levelup)

## Example - reading

read items in database.

``` js
var pl = require('pull-level')
var pull = require('pull-stream')

var db = require('levelup')('/tmp/pull-level-example')

pull(pl.read(db), pull.collect(console.log))
```

read items in database, plus realtime changes

``` js
pull(
  pl.read(db, {live: true}),
  //log data as it comes,
  //because tail will keep the connection open
  //so we'll never see the end otherwise.
  pull.through(console.log),
  //note, pull-streams will not drain unless something is
  //pulling the data through, so we have to add drain
  //even though the data we want is coming from pull.through()
  pull.drain()
)
```

If you just want the realtime inserts,
use `live`

``` js
pull(
  pl.live(db, {live: true}),
  pull.through(console.log),
  pull.drain()
)
```

## Example - writing

To write, pipe batch changes into `write`

``` js
pull(
  pull.values([
    {key: 0, value: 'zero', type: 'put'},
    {key: 1, value: 'one',  type: 'put'},
    {key: 2, value: 'two',  type: 'put'},
  ]),
  pl.write(db)
)
```

If you are lazy/busy, you can leave off `type`.
In that case, if `value` is non-null, the change
is considered a `put` else, a `del`.

``` js
pull(
  pull.values([
    {key: 0, value: 'zero'},
    {key: 1, value: 'one'},
    {key: 2, value: 'two'},
  ]), 
  pl.write(db)
)
```


## Example - indexes!

With pull-level it's easy to create indexes.
just save a pointer to the key.

like this:
``` js
pull(
  pull.values([
    {key: key, value: VALUE, type: 'put'},
    {key: '~INDEX~' + VALUE.prop, value: key,  type: 'put'},
  ]),
  pl.write(db)
)
```

then, when you want to do a `read`, use `asyncMap`

``` js
pull(
  pl.read(db, {min: '~INDEX~', max: '~INDEX~~'})
  pull.asyncMap(function (e, cb) {
    db.get(e.value, function (value) {
      cb(null, {key: e.value, value: value})
    })
  }),
  pull.collect(console.log)
)
```

## Example realtime aggregation

We want to keep a realtime count of everything in the database.
When ever something is inserted, we increment. But, we need
to check the records that are *currently* in the database.

Since it takes some time to scan the database, we need to make sure
we have done that before giving an answer. We can read it all with
one stream, using `{sync: true}` to be notified of when we have read out all the old records.

First all the old records are read from the non-live stream,
then you get one `{sync: true}` element, then all the new item.

``` js
var sum = 0, ready = false, waiting = []

//call get count to know s
function getSum (cb) {
  if(!ready) waiting.push(cb)
  else cb(null, sum)
}

pull(
  pl.read(db, {sync: true}),
  pull.drain(function (op) {
    if(op.sync) {
      //if we see a data element with this it means
      ready = true
      while(waiting.length) waiting.shift()(null, count)
    }
    //increment our counter!
    if(Number.isFinite(+op.value.amount)) //filter out non numbers & NaN.
      sum += op.value.amount
  })
)

```

## License

MIT
