var test = require('tape')
var path = require('path')
var leveldown = require('leveldown')
var iteratorStream = require('./')
var through2 = require('through2')

var db
var data = [
  { type: 'put', key: 'foobatch1', value: 'bar1' },
  { type: 'put', key: 'foobatch2', value: 'bar2' },
  { type: 'put', key: 'foobatch3', value: 'bar3' }
]

test('setup', function (t) {
  db = leveldown(path.join(__dirname, 'db-test'))
  db.open(function (err) {
    t.error(err, 'no error')
    db.batch(data, function (err) {
      t.error(err, 'no error')
      t.end()
    })
  })
})

test('keys and values', function (t) {
  var idx = 0
  var stream = iteratorStream(db.iterator())
  stream.pipe(through2.obj(function (kv, _, done) {
    t.ok(Buffer.isBuffer(kv.key))
    t.ok(Buffer.isBuffer(kv.value))
    t.equal(kv.key.toString(), data[idx].key)
    t.equal(kv.value.toString(), data[idx].value)
    idx++
    done()
  }, function () {
    t.equal(idx, data.length)
    stream.on('close', function () {
      t.end()
    })
  }))
})

test('.destroy closes the stream', function (t) {
  var stream = iteratorStream(db.iterator())
  stream.on('close', t.end.bind(t))
  stream.destroy()
})

test('.destroy during iterator.next 1', function (t) {
  var stream
  var iterator = db.iterator()
  var next = iterator.next.bind(iterator)
  iterator.next = function (cb) {
    t.pass('should be called once')
    next(cb)
    stream.destroy()
  }
  stream = iteratorStream(iterator)
  stream.on('data', function (data) {
    t.fail('should not be called')
  })
  stream.on('close', t.end.bind(t))
})

test('.destroy during iterator.next 2', function (t) {
  var stream
  var iterator = db.iterator()
  var next = iterator.next.bind(iterator)
  var count = 0
  iterator.next = function (cb) {
    t.pass('should be called')
    next(cb)
    if (++count === 2) {
      stream.destroy()
    }
  }
  stream = iteratorStream(iterator)
  stream.on('data', function (data) {
    t.pass('should be called')
  })
  stream.on('close', t.end.bind(t))
})

test('.destroy after iterator.next 1', function (t) {
  var stream
  var iterator = db.iterator()
  var next = iterator.next.bind(iterator)
  iterator.next = function (cb) {
    next(function (err, key, value) {
      stream.destroy()
      cb(err, key, value)
      t.pass('should be called')
    })
  }
  stream = iteratorStream(iterator)
  stream.on('data', function (data) {
    t.fail('should not be called')
  })
  stream.on('close', t.end.bind(t))
})

test('.destroy after iterator.next 2', function (t) {
  var stream
  var iterator = db.iterator()
  var next = iterator.next.bind(iterator)
  var count = 0
  iterator.next = function (cb) {
    next(function (err, key, value) {
      if (++count === 2) {
        stream.destroy()
      }
      cb(err, key, value)
      t.pass('should be called')
    })
  }
  stream = iteratorStream(iterator)
  stream.on('data', function (data) {
    t.pass('should be called')
  })
  stream.on('close', t.end.bind(t))
})

test('keys=false', function (t) {
  var stream = iteratorStream(db.iterator(), { keys: false })
  stream.once('data', function (value) {
    stream.destroy()
    t.equal(value.toString(), 'bar1')
    t.end()
  })
})

test('values=false', function (t) {
  var stream = iteratorStream(db.iterator(), { values: false })
  stream.once('data', function (key) {
    stream.destroy()
    t.equal(key.toString(), 'foobatch1')
    t.end()
  })
})
