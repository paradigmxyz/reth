var tape = require('tape')
var level = require('level')
var WriteStream = require('.')
var concat = require('level-concat-iterator')
var secretListener = require('secret-event-listener')
var tempy = require('tempy')

function monitor (stream) {
  var order = []

  ;['error', 'finish', 'close'].forEach(function (event) {
    secretListener(stream, event, function () {
      order.push(event)
    })
  })

  return order
}

function monkeyBatch (db, fn) {
  var down = db.db
  var original = down._batch.bind(down)
  down._batch = fn.bind(down, original)
}

function slowdown (db) {
  monkeyBatch(db, function (original, ops, options, cb) {
    setTimeout(function () {
      original(ops, options, cb)
    }, 500)
  })
}

function test (label, options, fn) {
  if (typeof options === 'function') {
    fn = options
    options = {}
  }

  options.createIfMissing = true
  options.errorIfExists = true

  tape(label, function (t) {
    var ctx = {}

    var sourceData = ctx.sourceData = []
    for (var i = 0; i < 2; i++) {
      ctx.sourceData.push({ key: String(i), value: 'value' })
    }

    ctx.verify = function (ws, done, data) {
      concat(ctx.db.iterator(), function (err, result) {
        t.error(err, 'no error')
        t.same(result, data || sourceData, 'correct data')
        done()
      })
    }

    level(tempy.directory(), options, function (err, db) {
      t.notOk(err, 'no error')
      ctx.db = db
      fn(t, ctx, function () {
        ctx.db.close(function (err) {
          t.notOk(err, 'no error')
          t.end()
        })
      })
    })
  })
}

// TODO: test various encodings

test('test simple WriteStream', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', ctx.verify.bind(ctx, ws, done))
  ctx.sourceData.forEach(function (d) {
    ws.write(d)
  })
  ws.end()
})

test('test WriteStream with async writes', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)
  var sourceData = ctx.sourceData
  var i = -1

  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', ctx.verify.bind(ctx, ws, done))

  function write () {
    if (++i >= sourceData.length) return ws.end()

    var d = sourceData[i]
    // some should batch() and some should put()
    if (d.key % 3) {
      setTimeout(function () {
        ws.write(d)
        process.nextTick(write)
      }, 10)
    } else {
      ws.write(d)
      process.nextTick(write)
    }
  }

  write()
})

test('race condition between batch callback and close event', function (t, ctx, done) {
  // Delaying the batch should not be a problem
  slowdown(ctx.db)

  var ws = WriteStream(ctx.db)
  var i = 0

  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', ctx.verify.bind(ctx, ws, done))
  ctx.sourceData.forEach(function (d) {
    i++
    if (i < ctx.sourceData.length) {
      ws.write(d)
    } else {
      ws.end(d)
    }
  })
})

test('race condition between two flushes', function (t, ctx, done) {
  slowdown(ctx.db)

  var ws = WriteStream(ctx.db)
  var order = monitor(ws)

  ws.on('close', function () {
    t.same(order, ['batch', 'batch', 'close'])

    ctx.verify(ws, done, [
      { key: 'a', value: 'a' },
      { key: 'b', value: 'b' }
    ])
  })

  ctx.db.on('batch', function () {
    order.push('batch')
  })

  ws.write({ key: 'a', value: 'a' })

  // Schedule another flush while the first is in progress
  ctx.db.once('batch', function (ops) {
    ws.end({ key: 'b', value: 'b' })
  })
})

test('test end accepts data', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)
  var i = 0

  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', ctx.verify.bind(ctx, ws, done))
  ctx.sourceData.forEach(function (d) {
    i++
    if (i < ctx.sourceData.length) {
      ws.write(d)
    } else {
      ws.end(d)
    }
  })
})

test('test destroy()', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)

  var verify = function () {
    concat(ctx.db.iterator(), function (err, result) {
      t.error(err, 'no error')
      t.same(result, [], 'results should be empty')
      done()
    })
  }

  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', verify.bind(null))
  ctx.sourceData.forEach(function (d) {
    ws.write(d)
  })
  ws.destroy()
})

test('test destroy(err)', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)
  var order = monitor(ws)

  ws.on('error', function (err) {
    t.is(err.message, 'user error', 'got error')
  })

  ws.on('close', function () {
    t.same(order, ['error', 'close'])

    concat(ctx.db.iterator(), function (err, result) {
      t.error(err, 'no error')
      t.same(result, [], 'results should be empty')
      done()
    })
  })

  ctx.sourceData.forEach(function (d) {
    ws.write(d)
  })

  ws.destroy(new Error('user error'))
})

test('test json encoding', { keyEncoding: 'utf8', valueEncoding: 'json' }, function (t, ctx, done) {
  var data = [
    { key: 'aa', value: { a: 'complex', obj: 100 } },
    { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ba', value: { a: 'complex', obj: 100 } },
    { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ca', value: { a: 'complex', obj: 100 } },
    { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
  ]

  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', ctx.verify.bind(ctx, ws, done, data))
  data.forEach(function (d) {
    ws.write(d)
  })
  ws.end()
})

test('test del capabilities for each key/value', { keyEncoding: 'utf8', valueEncoding: 'json' }, function (t, ctx, done) {
  var data = [
    { key: 'aa', value: { a: 'complex', obj: 100 } },
    { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ba', value: { a: 'complex', obj: 100 } },
    { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ca', value: { a: 'complex', obj: 100 } },
    { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
  ]

  function del () {
    var delStream = WriteStream(ctx.db)
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify()
    })
    data.forEach(function (d) {
      d.type = 'del'
      delStream.write(d)
    })

    delStream.end()
  }

  function verify () {
    concat(ctx.db.iterator(), function (err, result) {
      t.error(err, 'no error')
      t.same(result, [], 'results should be empty')
      done()
    })
  }

  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', function () {
    del()
  })
  data.forEach(function (d) {
    ws.write(d)
  })
  ws.end()
})

test('test del capabilities as constructor option', { keyEncoding: 'utf8', valueEncoding: 'json' }, function (t, ctx, done) {
  var data = [
    { key: 'aa', value: { a: 'complex', obj: 100 } },
    { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ba', value: { a: 'complex', obj: 100 } },
    { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ca', value: { a: 'complex', obj: 100 } },
    { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
  ]

  function del () {
    var delStream = WriteStream(ctx.db, { type: 'del' })
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify()
    })
    data.forEach(function (d) {
      delStream.write(d)
    })

    delStream.end()
  }

  function verify () {
    concat(ctx.db.iterator(), function (err, result) {
      t.error(err, 'no error')
      t.same(result, [], 'results should be empty')
      done()
    })
  }

  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', function () {
    del()
  })
  data.forEach(function (d) {
    ws.write(d)
  })
  ws.end()
})

test('test type at key/value level must take precedence on the constructor', { keyEncoding: 'utf8', valueEncoding: 'json' }, function (t, ctx, done) {
  var data = [
    { key: 'aa', value: { a: 'complex', obj: 100 } },
    { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ba', value: { a: 'complex', obj: 100 } },
    { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } },
    { key: 'ca', value: { a: 'complex', obj: 100 } },
    { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } },
    { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
  ]
  var exception = data[0]

  exception['type'] = 'put'

  function del () {
    var delStream = WriteStream(ctx.db, { type: 'del' })
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify()
    })
    data.forEach(function (d) {
      delStream.write(d)
    })

    delStream.end()
  }

  function verify () {
    concat(ctx.db.iterator(), function (err, result) {
      t.error(err, 'no error')
      var expected = [ { key: data[0].key, value: data[0].value } ]
      t.same(result, expected, 'only one element')
      done()
    })
  }

  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.notOk(err, 'no error')
  })
  ws.on('close', function () {
    del()
  })
  data.forEach(function (d) {
    ws.write(d)
  })
  ws.end()
})

test('test that missing type errors', function (t, ctx, done) {
  var data = { key: 314, type: 'foo' }
  var errored = false

  function verify () {
    ctx.db.get(data.key, function (err, value) {
      t.equal(errored, true, 'error received in stream')
      t.ok(err, 'got expected error')
      t.equal(err.notFound, true, 'not found error')
      t.notOk(value, 'did not get value')
      done()
    })
  }

  var ws = WriteStream(ctx.db)
  ws.on('error', function (err) {
    t.equal(err.message, '`type` must be \'put\' or \'del\'', 'should error')
    errored = true
  })
  ws.on('close', function () {
    verify()
  })
  ws.write(data)
  ws.end()
})

test('test limbo batch error', function (t, ctx, done) {
  var ws = WriteStream(ctx.db)
  var order = monitor(ws)

  monkeyBatch(ctx.db, function (original, ops, options, cb) {
    process.nextTick(cb, new Error('batch error'))
  })

  ws.on('error', function (err) {
    t.is(err.message, 'batch error')
  })

  ws.on('close', function () {
    t.same(order, ['error', 'close'])
    t.end()
  })

  // Don't end(), because we want the error to follow a
  // specific code path (when there is no _flush listener).
  ws.write({ key: 'a', value: 'a' })
})

test('test batch error when buffer is full', function (t, ctx, done) {
  var ws = WriteStream(ctx.db, { maxBufferLength: 1 })
  var order = monitor(ws)

  monkeyBatch(ctx.db, function (original, ops, options, cb) {
    process.nextTick(cb, new Error('batch error'))
  })

  ws.on('error', function (err) {
    t.is(err.message, 'batch error', 'got error')
  })

  ws.on('close', function () {
    t.same(order, ['error', 'close'])
    t.end()
  })

  // Don't end(), because we want the error to follow a
  // specific code path (when we're waiting to drain).
  ws.write({ key: 'a', value: 'a' })
  ws.write({ key: 'b', value: 'b' })
})

test('test destroy while waiting to drain', function (t, ctx, done) {
  var ws = WriteStream(ctx.db, { maxBufferLength: 1 })
  var order = monitor(ws)

  ws.on('error', function (err) {
    t.is(err.message, 'user error', 'got error')
  })

  ws.on('close', function () {
    t.same(order, ['error', 'close'])
    t.end()
  })

  ws.prependListener('_flush', function (err) {
    t.ifError(err, 'no _flush error')
    ws.destroy(new Error('user error'))
  })

  // Don't end.
  ws.write({ key: 'a', value: 'a' })
  ws.write({ key: 'b', value: 'b' })
})

;[0, 1, 2, 10, 20, 100].forEach(function (max) {
  test('test maxBufferLength: ' + max, testMaxBuffer(max, false))
  test('test maxBufferLength: ' + max + ' (random)', testMaxBuffer(max, true))
})

function testMaxBuffer (max, randomize) {
  return function (t, ctx, done) {
    var ws = WriteStream(ctx.db, { maxBufferLength: max })
    var sourceData = []
    var batches = []

    for (var i = 0; i < 20; i++) {
      sourceData.push({ key: i < 10 ? '0' + i : String(i), value: 'value' })
    }

    var expectedSize = max || sourceData.length
    var remaining = sourceData.slice()

    ws.on('close', function () {
      t.ok(batches.every(function (size, index) {
        // Last batch may contain additional items
        return size <= expectedSize || index === batches.length - 1
      }), 'batch sizes are <= max')

      ctx.verify(ws, done, sourceData)
    })

    ctx.db.on('batch', function (ops) {
      batches.push(ops.length)
    })

    loop()

    function loop () {
      var toWrite = randomize
        ? Math.floor(Math.random() * remaining.length + 1)
        : remaining.length

      remaining.splice(0, toWrite).forEach(function (d) {
        ws.write(d)
      })

      if (remaining.length) {
        setImmediate(loop)
      } else {
        ws.end()
      }
    }
  }
}
