var pull = require('pull-stream/pull')
var looper = require('looper')

function destroy (stream) {
  if(!stream.destroy)
    console.error(
      'warning, stream-to-pull-stream: \n'
    + 'the wrapped node-stream does not implement `destroy`, \n'
    + 'this may cause resource leaks.'
    )
  else stream.destroy()

}

function write(read, stream, cb) {
  var ended, closed = false, did
  function done () {
    if(did) return
    did = true
    cb && cb(ended === true ? null : ended)
  }

  function onClose () {
    if(closed) return
    closed = true
    cleanup()
    if(!ended) read(ended = true, done)
    else       done()
  }
  function onError (err) {
    cleanup()
    if(!ended) read(ended = err, done)
  }
  function cleanup() {
    stream.on('finish', onClose)
    stream.removeListener('close', onClose)
    stream.removeListener('error', onError)
  }
  stream.on('close', onClose)
  stream.on('finish', onClose)
  stream.on('error', onError)
  process.nextTick(function () {
    looper(function (next) {
      read(null, function (end, data) {
        ended = ended || end
        //you can't "end" a stdout stream, so this needs to be handled specially.
        if(end === true)
          return stream._isStdio ? done() : stream.end()

        if(ended = ended || end) {
          destroy(stream)
          return done(ended)
        }

        //I noticed a problem streaming to the terminal:
        //sometimes the end got cut off, creating invalid output.
        //it seems that stdout always emits "drain" when it ends.
        //so this seems to work, but i have been unable to reproduce this test
        //automatically, so you need to run ./test/stdout.js a few times and the end is valid json.
        if(stream._isStdio)
          stream.write(data, function () { next() })
        else {
          var pause = stream.write(data)
          if(pause === false)
            stream.once('drain', next)
          else next()
        }
      })
    })
  })
}

function first (emitter, events, handler) {
  function listener (val) {
    events.forEach(function (e) {
      emitter.removeListener(e, listener)
    })
    handler(val)
  }
  events.forEach(function (e) {
    emitter.on(e, listener)
  })
  return emitter
}

function read2(stream) {
  var ended = false, waiting = false
  var _cb

  function read () {
    var data = stream.read()
    if(data !== null && _cb) {
      var cb = _cb; _cb = null
      cb(null, data)
    }
  }

  stream.on('readable', function () {
    waiting = true
    _cb && read()
  })
  .on('end', function () {
    ended = true
    _cb && _cb(ended)
  })
  .on('error', function (err) {
    ended = err
    _cb && _cb(ended)
  })

  return function (end, cb) {
    _cb = cb
    if(ended)
      cb(ended)
    else if(waiting)
      read()
  }
}

function read1(stream) {
  var buffer = [], cbs = [], ended, paused = false

  var draining
  function drain() {
    while((buffer.length || ended) && cbs.length)
      cbs.shift()(buffer.length ? null : ended, buffer.shift())
    if(!buffer.length && (paused)) {
      paused = false
      stream.resume()
    }
  }

  stream.on('data', function (data) {
    buffer.push(data)
    drain()
    if(buffer.length && stream.pause) {
      paused = true
      stream.pause()
    }
  })
  stream.on('end', function () {
    ended = true
    drain()
  })
  stream.on('close', function () {
    ended = true
    drain()
  })
  stream.on('error', function (err) {
    ended = err
    drain()
  })
  return function (abort, cb) {
    if(!cb) throw new Error('*must* provide cb')
    if(abort) {
      function onAbort () {
        while(cbs.length) cbs.shift()(abort)
        cb(abort)
      }
      //if the stream happens to have already ended, then we don't need to abort.
      if(ended) return onAbort()
      stream.once('close', onAbort)
      destroy(stream)
    }
    else {
      cbs.push(cb)
      drain()
    }
  }
}

var read = read1

var sink = function (stream, cb) {
  return function (read) {
    return write(read, stream, cb)
  }
}

var source = function (stream) {
  return read1(stream)
}

exports = module.exports = function (stream, cb) {
  return (
    (stream.writable && stream.write)
    ? stream.readable
      ? function(_read) {
          write(_read, stream, cb);
          return read1(stream)
        }
      : sink(stream, cb)
    : source(stream)
  )
}

exports.sink = sink
exports.source = source
exports.read = read
exports.read1 = read1
exports.read2 = read2
exports.duplex = function (stream, cb) {
  return {
    source: source(stream),
    sink: sink(stream, cb)
  }
}
exports.transform = function (stream) {
  return function (read) {
    var _source = source(stream)
    sink(stream)(read); return _source
  }
}









