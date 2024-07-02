# pull-stream

Minimal Pipeable Pull-stream

In [classic-streams](https://github.com/nodejs/node-v0.x-archive/blob/v0.8/doc/api/stream.markdown),
streams _push_ data to the next stream in the pipeline.
In [new-streams](https://github.com/nodejs/node-v0.x-archive/blob/v0.10/doc/api/stream.markdown),
data is pulled out of the source stream, into the destination.
`pull-stream` is a minimal take on streams,
pull streams work great for "object" streams as well as streams of raw text or binary data.

[![build status](https://secure.travis-ci.org/pull-stream/pull-stream.png)](https://travis-ci.org/pull-stream/pull-stream)


## Quick Example

Stat some files:

```js
pull(
  pull.values(['file1', 'file2', 'file3']),
  pull.asyncMap(fs.stat),
  pull.collect(function (err, array) {
    console.log(array)
  })
)
```
Note that `pull(a, b, c)` is basically the same as `a.pipe(b).pipe(c)`.

To grok how pull-streams work, read through [pull-streams workshop](https://github.com/pull-stream/pull-stream-workshop)

## How do I do X with pull-streams?

There is a module for that!

Check the [pull-stream FAQ](https://github.com/pull-stream/pull-stream-faq)
and post an issue if you have a question that is not covered.

## Compatibily with node streams

pull-streams are not _directly_ compatible with node streams,
but pull-streams can be converted into node streams with
[pull-stream-to-stream](https://github.com/pull-stream/pull-stream-to-stream)
and node streams can be converted into pull-stream using [stream-to-pull-stream](https://github.com/pull-stream/stream-to-pull-stream)
correct back pressure is preserved.

### Readable & Reader vs. Readable & Writable

Instead of a readable stream, and a writable stream, there is a `readable` stream,
 (aka "Source") and a `reader` stream (aka "Sink"). Through streams
is a Sink that returns a Source.

See also:
* [Sources](./docs/sources/index.md)
* [Throughs](./docs/throughs/index.md)
* [Sinks](./docs/sinks/index.md)

### Source (readable stream that produces values)

A Source is a function `read(end, cb)`,
that may be called many times,
and will (asynchronously) call `cb(null, data)` once for each call.

To signify an end state, the stream eventually returns `cb(err)` or `cb(true)`.
When signifying an end state, `data` *must* be ignored.

The `read` function *must not* be called until the previous call has called back.
Unless, it is a call to abort the stream (`read(Error || true, cb)`).

```js
var n = 5;

// random is a source 5 of random numbers.
function random (end, cb) {
  if(end) return cb(end)
  // only read n times, then stop.
  if(0 > --n) return cb(true)
  cb(null, Math.random())
}

```

### Sink (reader or writable stream that consumes values)

A Sink is a function `reader(read)` that calls a Source (`read(null, cb)`),
until it decides to stop (by calling `read(true, cb)`), or the readable ends (`read` calls
`cb(Error || true)`

All [Throughs](./docs/throughs/index.md)
and [Sinks](./docs/sinks/index.md)
are reader streams.

```js
// logger reads a source and logs it.
function logger (read) {
  read(null, function next(end, data) {
    if(end === true) return
    if(end) throw end

    console.log(data)
    read(null, next)
  })
}
```

Since Sources and Sinks are functions, you can pass them to each other!

```js
logger(random) //"pipe" the streams.

```

but, it's easier to read if you use's pull-stream's `pull` method

```js
var pull = require('pull-stream')

pull(random, logger)
```

### Creating reusable streams

When working with pull streams it is common to create functions that return a stream.
This is because streams contain mutable state and so can only be used once. 
In the above example, once `random`  has been connected to a sink and has produced 5 random numbers it will not produce any more random numbers if connected to another sink.

Therefore, use a function like this to create a random number generating stream that can be reused:

```js

// create a stream of n random numbers
function createRandomStream (n) {
  return function randomReadable (end, cb) {
    if(end) return cb(end)
    if(0 > --n) return cb(true)
    cb(null, Math.random())
  }
}

pull(createRandomStream(5), logger)
```


### Through

A through stream is both a reader (consumes values) and a readable (produces values).
It's a function that takes a `read` function (a Sink),
and returns another `read` function (a Source).

```js
// double is a through stream that doubles values.
function double (read) {
  return function readable (end, cb) {
    read(end, function (end, data) {
      cb(end, data != null ? data * 2 : null)
    })
  }
}

pull(createRandomStream(5), double, logger)
```

### Pipeability

Every pipeline must go from a `source` to a `sink`.
Data will not start moving until the whole thing is connected.

```js
pull(source, through, sink)
```

some times, it's simplest to describe a stream in terms of other streams.
pull can detect what sort of stream it starts with (by counting arguments)
and if you pull together through streams, it gives you a new through stream.

```js
var tripleThrough =
  pull(through1(), through2(), through3())
// The three through streams become one.

pull(source(), tripleThrough, sink())
```

pull detects if it's missing a Source by checking function arity,
if the function takes only one argument it's either a sink or a through.
Otherwise it's a Source.

## Duplex Streams

Duplex streams, which are used to communicate between two things,
(i.e. over a network) are a little different. In a duplex stream,
messages go both ways, so instead of a single function that represents the stream,
you need a pair of streams. `{source: sourceStream, sink: sinkStream}`

Pipe duplex streams like this:

``` js
var a = duplex()
var b = duplex()

pull(a.source, b.sink)
pull(b.source, a.sink)

//which is the same as

b.sink(a.source); a.sink(b.source)

//but the easiest way is to allow pull to handle this

pull(a, b, a)

//"pull from a to b and then back to a"

```

This means two duplex streams communicating actually forms two completely
independent pipelines:
1. Side1's sink pulling from Side2's source
2. Side2's sink pulling from Side1's source

As a result, one pipeline might finish or error out before or after the other.
A duplex stream is only "finished" once both pipelines are done communicating.


## Design Goals & Rationale

There is a deeper,
[platonic abstraction](http://en.wikipedia.org/wiki/Platonic_idealism),
where a streams is just an array in time, instead of in space.
And all the various streaming "abstractions" are just crude implementations
of this abstract idea.

[classic-streams](https://github.com/joyent/node/blob/v0.8.16/doc/api/stream.markdown),
[new-streams](https://github.com/joyent/node/blob/v0.10/doc/api/stream.markdown),
[reducers](https://github.com/Gozala/reducers)

The objective here is to find a simple realization of the best features of the above.

### Type Agnostic

A stream abstraction should be able to handle both streams of text and streams
of objects.

### A pipeline is also a stream.

Something like this should work: `a.pipe(x.pipe(y).pipe(z)).pipe(b)`
this makes it possible to write a custom stream simply by
combining a few available streams.

### Propagate End/Error conditions.

If a stream ends in an unexpected way (error),
then other streams in the pipeline should be notified.
(this is a problem in node streams - when an error occurs,
the stream is disconnected, and the user must handle that specially)

Also, the stream should be able to be ended from either end.

### Transparent Backpressure & Laziness

Very simple transform streams must be able to transfer back pressure
instantly.

This is a problem in node streams, pause is only transfered on write, so
on a long chain (`a.pipe(b).pipe(c)`), if `c` pauses, `b` will have to write to it
to pause, and then `a` will have to write to `b` to pause.
If `b` only transforms `a`'s output, then `a` will have to write to `b` twice to
find out that `c` is paused.

[reducers](https://github.com/Gozala/reducers) reducers has an interesting method,
where synchronous tranformations propagate back pressure instantly!

This means you can have two "smart" streams doing io at the ends, and lots of dumb
streams in the middle, and back pressure will work perfectly, as if the dumb streams
are not there.

This makes laziness work right.

### handling end, error, and abort.

in pull streams, any part of the stream (source, sink, or through)
may terminate the stream. (this is the case with node streams too,
but it's not handled well).

#### source: end, error

A source may end (`cb(true)` after read) or error (`cb(error)` after read)
After ending, the source *must* never `cb(null, data)`

#### sink: abort

Sinks do not normally end the stream, but if they decide they do
not need any more data they may "abort" the source by calling `read(true, cb)`.
A abort (`read(true, cb)`) may be called before a preceding read call
has called back.

### handling end/abort/error in through streams

Rules for implementing `read` in a through stream:
1) Sink wants to stop. sink aborts the through

    just forward the exact read() call to your source,
    any future read calls should cb(true).

2) We want to stop. (abort from the middle of the stream)

    abort your source, and then cb(true) to tell the sink we have ended.
    If the source errored during abort, end the sink by cb read with `cb(err)`.
    (this will be an ordinary end/error for the sink)

3) Source wants to stop. (`read(null, cb) -> cb(err||true)`)

    forward that exact callback towards the sink chain,
    we must respond to any future read calls with `cb(err||true)`.

In none of the above cases data is flowing!
4) If data is flowing (normal operation:   `read(null, cb) -> cb(null, data)`

    forward data downstream (towards the Sink)
    do none of the above!

There either is data flowing (4) OR you have the error/abort cases (1-3), never both.


## 1:1 read-callback ratio

A pull stream source (and thus transform) returns *exactly one value* per read.

This differs from node streams, which can use `this.push(value)` and in internal
buffer to create transforms that write many values from a single read value.

Pull streams don't come with their own buffering mechanism, but [there are ways
to get around this](https://github.com/dominictarr/pull-stream-examples/blob/master/buffering.js).


## Minimal bundle

If you need only the `pull` function from this package you can reduce the size
of the imported code (for instance to reduce a Browserify bundle) by requiring
it directly:


```js
var pull = require('pull-stream/pull')

pull(createRandomStream(5), logger())
```


## Further Examples

- [dominictarr/pull-stream-examples](https://github.com/dominictarr/pull-stream-examples)
- [./docs/examples](./docs/examples.md)

Explore this repo further for more information about
[sources](./docs/sources/index.md),
[throughs](./docs/throughs/index.md),
[sinks](./docs/sinks/index.md), and
[glossary](./docs/glossary.md).


## License

MIT


