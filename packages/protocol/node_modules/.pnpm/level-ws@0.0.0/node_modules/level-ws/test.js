/* Copyright (c) 2012-2013 LevelUP contributors
 * See list at <https://github.com/rvagg/node-levelup#contributing>
 * MIT +no-false-attribs License <https://github.com/rvagg/node-levelup/blob/master/LICENSE>
 */

var after  = require('after')
  , tape   = require('tape')
  , os     = require('os')
  , path   = require('path')
  , fs     = require('fs')
  , level  = require('level')
  , rimraf = require('rimraf')
  , ws     = require('./')

function cleanup (callback) {
  fs.readdir(os.tmpdir(), function (err, list) {
    if (err) return callback(err)

    list = list.filter(function (f) {
      return (/^_level-ws_test_db\./).test(f)
    })

    if (!list.length)
      return callback()

    var ret = 0

    list.forEach(function (f) {
      rimraf(path.join(__dirname, f), function () {
        if (++ret == list.length)
          callback()
      })
    })
  })
}

function openTestDatabase (t, options, callback) {
  var location = path.join(os.tmpdir(), '_level-ws_test_db.' + Math.random())
  if (typeof options == 'function') {
    callback = options
    options  = { createIfMissing: true, errorIfExists: true }
  }

  rimraf(location, function (err) {
    t.notOk(err, 'no error')
    level(location, options, function (err, db) {
      t.notOk(err, 'no error')
      if (!err) {
        this.db = ws(db) // invoke ws!
        callback(this.db)
      }
    }.bind(this))
  }.bind(this))
}

function setUp (t) {
  this.openTestDatabase = openTestDatabase.bind(this, t)

  this.timeout = 1000

  this.sourceData = []

  for (var i = 0; i < 10; i++) {
    this.sourceData.push({
        type  : 'put'
      , key   : i
      , value : Math.random()
    })
  }

  this.verify = function (ws, db, done, data) {
    if (!data) data = this.sourceData // can pass alternative data array for verification
    t.ok(ws.writable === false, 'not writable')
    t.ok(ws.readable === false, 'not readable')
    var _done = after(data.length, done)
    data.forEach(function (data) {
      db.get(data.key, function (err, value) {
        t.notOk(err, 'no error')
        if (typeof value == 'object')
          t.deepEqual(value, data.value, 'WriteStream data #' + data.key + ' has correct value')
        else
          t.equal(+value, +data.value, 'WriteStream data #' + data.key + ' has correct value')
        _done()
      })
    })
  }
}


function test (label, fn) {
  tape(label, function (t) {
    var ctx = {}
    setUp.call(ctx, t)
    fn.call(ctx, t, function () {
      var _cleanup = cleanup.bind(ctx, t.end.bind(t))
      if (ctx.db)
        return ctx.db.close(_cleanup)
      _cleanup()
    })
  })
}

//TODO: test various encodings

test('test simple WriteStream', function (t, done) {
  this.openTestDatabase(function (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', this.verify.bind(this, ws, db, done))
    this.sourceData.forEach(function (d) {
      ws.write(d)
    })
    ws.end()
  }.bind(this))
})

test('test WriteStream with async writes', function (t, done) {
  this.openTestDatabase(function (db) {
    var ws         = db.createWriteStream()
      , sourceData = this.sourceData
      , i          = -1

    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', this.verify.bind(this, ws, db, done))

    function write () {
      if (++i >= sourceData.length)
        return ws.end()

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
  }.bind(this))
})

test('test end accepts data', function (t, done) {
  this.openTestDatabase(function (db) {
    var ws = db.createWriteStream()
      , i  = 0

    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', this.verify.bind(this, ws, db, done))
    this.sourceData.forEach(function (d) {
      i++
      if (i < this.sourceData.length) {
        ws.write(d)
      } else {
        ws.end(d)
      }
    }.bind(this))
  }.bind(this))
})

// at the moment, destroySoon() is basically just end()
test('test destroySoon()', function (t, done) {
  this.openTestDatabase(function (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', this.verify.bind(this, ws, db, done))
    this.sourceData.forEach(function (d) {
      ws.write(d)
    })
    ws.destroySoon()
  }.bind(this))
})

test('test destroy()', function (t, done) {
  var verify = function (ws, db) {
    t.ok(ws.writable === false, 'not writable')
    var _done = after(this.sourceData.length, done)
    this.sourceData.forEach(function (data) {
      db.get(data.key, function (err, value) {
        // none of them should exist
        t.ok(err, 'got expected error')
        t.notOk(value, 'did not get value')
        _done()
      })
    })
  }

  this.openTestDatabase(function (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    t.ok(ws.writable === true, 'is writable')
    t.ok(ws.readable === false, 'not readable')
    ws.on('close', verify.bind(this, ws, db))
    this.sourceData.forEach(function (d) {
      ws.write(d)
      t.ok(ws.writable === true, 'is writable')
      t.ok(ws.readable === false, 'not readable')
    })
    t.ok(ws.writable === true, 'is writable')
    t.ok(ws.readable === false, 'not readable')
    ws.destroy()
  }.bind(this))
})

test('test json encoding', function (t, done) {
  var options = { createIfMissing: true, errorIfExists: true, keyEncoding: 'utf8', valueEncoding: 'json' }
    , data = [
          { type: 'put', key: 'aa', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { type: 'put', key: 'ba', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { type: 'put', key: 'ca', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
      ]

  this.openTestDatabase(options, function (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', this.verify.bind(this, ws, db, done, data))
    data.forEach(function (d) {
      ws.write(d)
    })
    ws.end()
  }.bind(this))
})

test('test del capabilities for each key/value', function (t, done) {
  var options = { createIfMissing: true, errorIfExists: true, keyEncoding: 'utf8', valueEncoding: 'json' }
    , data = [
          { type: 'put', key: 'aa', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { type: 'put', key: 'ba', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { type: 'put', key: 'ca', value: { a: 'complex', obj: 100 } }
        , { type: 'put', key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { type: 'put', key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
      ]
    , self = this

  function open () {
    self.openTestDatabase(options, function (db) {
      write(db);
    });
  }

  function write (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', function () {
      del(db);
    })
    data.forEach(function (d) {
      ws.write(d)
    })

    ws.end()
  }

  function del (db) {
    var delStream = db.createWriteStream()
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify(db);
    })
    data.forEach(function (d) {
      d.type = 'del'
      delStream.write(d)
    })

    delStream.end()
  }

  function verify (db) {
    var _done = after(data.length, done)
    data.forEach(function (data) {
      db.get(data.key, function (err, value) {
        // none of them should exist
        t.ok(err, 'got expected error')
        t.notOk(value, 'did not get value')
        _done()
      })
    })
  }

  open()
})

test('test del capabilities as constructor option', function (t, done) {

  var options = { createIfMissing: true, errorIfExists: true, keyEncoding: 'utf8', valueEncoding: 'json' }
    , data = [
          { key: 'aa', value: { a: 'complex', obj: 100 } }
        , { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { key: 'ba', value: { a: 'complex', obj: 100 } }
        , { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { key: 'ca', value: { a: 'complex', obj: 100 } }
        , { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
      ]
    , self = this

  function open () {
    self.openTestDatabase(options, function (db) {
      write(db);
    });
  }

  function write (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', function () {
      del(db);
    })
    data.forEach(function (d) {
      ws.write(d)
    })

    ws.end()
  }

  function del (db) {
    var delStream = db.createWriteStream({ type: 'del' })
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify(db);
    })
    data.forEach(function (d) {
      delStream.write(d)
    })

    delStream.end()
  }

  function verify (db) {
    var _done = after(data.length, done)
    data.forEach(function (data) {
      db.get(data.key, function (err, value) {
        // none of them should exist
        t.ok(err, 'got expected error')
        t.notOk(value, 'did not get value')
        _done()
      })
    })
  }

  open()
})

test('test type at key/value level must take precedence on the constructor', function (t, done) {
  var options = { createIfMissing: true, errorIfExists: true, keyEncoding: 'utf8', valueEncoding: 'json' }
    , data = [
          { key: 'aa', value: { a: 'complex', obj: 100 } }
        , { key: 'ab', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'ac', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { key: 'ba', value: { a: 'complex', obj: 100 } }
        , { key: 'bb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'bc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
        , { key: 'ca', value: { a: 'complex', obj: 100 } }
        , { key: 'cb', value: { b: 'foo', bar: [ 1, 2, 3 ] } }
        , { key: 'cc', value: { c: 'w00t', d: { e: [ 0, 10, 20, 30 ], f: 1, g: 'wow' } } }
      ]
    , exception = data[0]
    , self = this

  exception['type'] = 'put'

  function open () {
    self.openTestDatabase(options, function (db) {
      write(db);
    });
  }

  function write (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', function () {
      del(db);
    })
    data.forEach(function (d) {
      ws.write(d)
    })

    ws.end()
  }

  function del (db) {
    var delStream = db.createWriteStream({ type: 'del' })
    delStream.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    delStream.on('close', function () {
      verify(db);
    })
    data.forEach(function (d) {
      delStream.write(d)
    })

    delStream.end()
  }

  function verify (db) {
    var _done = after(data.length, done)
    data.forEach(function (data) {
      db.get(data.key, function (err, value) {
        if (data.type === 'put') {
          t.ok(value, 'got value')
          _done()
        } else {
          t.ok(err, 'got expected error')
          t.notOk(value, 'did not get value')
          _done()
        }
      })
    })
  }

  open()
})

test('test ignoring pairs with the wrong type', function (t, done) {
  var self = this

  function open () {
    self.openTestDatabase(write)
  }

  function write (db) {
    var ws = db.createWriteStream()
    ws.on('error', function (err) {
      t.notOk(err, 'no error')
    })
    ws.on('close', function () {
      verify(db)
    })
    self.sourceData.forEach(function (d) {
      d.type = 'x' + Math.random()
      ws.write(d)
    })
    ws.end()
  }

  function verify (db) {
    var _done = after(self.sourceData.length, done)
    self.sourceData.forEach(function (data) {
      db.get(data.key, function (err, value) {
        t.ok(err, 'got expected error')
        t.notOk(value, 'did not get value')
        _done()
      })
    })
  }

  open()
})
