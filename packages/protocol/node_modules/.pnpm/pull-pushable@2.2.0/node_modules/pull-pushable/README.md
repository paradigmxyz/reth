# pull-pushable

A pull-stream with a pushable interface.

Use this when you really can't pull from your source.
For example, often I like to have a "live" stream.
This would read a series of data, first old data,
but then stay open and read new data as it comes in.

In that case, the new data needs to be queued up while the old data is read,
and also, the rate things are pushed into the queue doesn't affect the rate of reads.

If there is no realtime aspect to this stream, it's likely that you don't need pushable.
Instead try just using `pull.values(array)`.

## Example

```js
var Pushable = require('pull-pushable')
var pull     = require('pull-stream')
var p = Pushable()

pull(p, pull.drain(console.log))

p.push(1)
p.end()
```

Also, can provide a listener for when the stream is closed.

```js
var Pushable = require('pull-pushable')
var pull     = require('pull-stream')
var p = Pushable(function (err) {
  console.log('stream closed!')
})

//read 3 times then abort.
pull(p, pull.take(3), pull.drain(console.log))

p.push(1)
p.push(2)
p.push(3)
p.push(4) //stream will be aborted before this is output
```

When giving the stream away and you don't want the user to have the `push`/`end` functions,
you can pass a `separated` option.  It returns `{ push, end, source, buffer }`.

```js
function createStream () {
  var p = Pushable(true) // optionally pass `onDone` after it

  somethingAsync((err, data) => {
    if (err) return p.end(err)
    p.push(data)
  })

  return p.source
}

var stream = createStream()
// stream.push === undefined
```

The current buffer array is exposed as `buffer` if you need to inspect or
manipulate it.

## License

MIT

