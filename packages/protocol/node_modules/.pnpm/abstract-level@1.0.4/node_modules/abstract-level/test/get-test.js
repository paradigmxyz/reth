'use strict'

const isBuffer = require('is-buffer')
const { verifyNotFoundError, illegalKeys, assertAsync } = require('./util')

let db

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

exports.args = function (test, testCommon) {
  test('test get() with illegal keys', assertAsync.ctx(function (t) {
    t.plan(illegalKeys.length * 5)

    for (const { name, key } of illegalKeys) {
      db.get(key, assertAsync(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (callback)')
      }))

      db.get(key).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (promise)')
      })
    }
  }))
}

exports.get = function (test, testCommon) {
  test('test simple get()', function (t) {
    db.put('foo', 'bar', function (err) {
      t.error(err)
      db.get('foo', function (err, value) {
        t.error(err)
        t.is(value, 'bar')

        db.get('foo', {}, function (err, value) { // same but with {}
          t.error(err)
          t.is(value, 'bar')

          db.get('foo', { valueEncoding: 'utf8' }, function (err, value) {
            t.error(err)
            t.is(value, 'bar')

            if (!db.supports.encodings.buffer) {
              return t.end()
            }

            db.get('foo', { valueEncoding: 'buffer' }, function (err, value) {
              t.error(err)
              t.ok(isBuffer(value), 'should be buffer')
              t.is(value.toString(), 'bar')
              t.end()
            })
          })
        })
      })
    })
  })

  test('test get() with promise', function (t) {
    db.put('promises', 'yes', function (err) {
      t.error(err)

      db.get('promises').then(function (value) {
        t.is(value, 'yes', 'got value without options')

        db.get('not found').catch(function (err) {
          t.ok(err, 'should error')
          t.ok(verifyNotFoundError(err), 'correct error')

          if (!db.supports.encodings.buffer) {
            return t.end()
          }

          db.get('promises', { valueEncoding: 'buffer' }).then(function (value) {
            t.ok(isBuffer(value), 'is buffer')
            t.is(value.toString(), 'yes', 'correct value')
            t.end()
          }).catch(t.fail.bind(t))
        })
      }).catch(t.fail.bind(t))
    })
  })

  test('test simultaneous get()', function (t) {
    db.put('hello', 'world', function (err) {
      t.error(err)
      let completed = 0
      const done = function () {
        if (++completed === 20) t.end()
      }

      for (let i = 0; i < 10; ++i) {
        db.get('hello', function (err, value) {
          t.error(err)
          t.is(value.toString(), 'world')
          done()
        })
      }

      for (let i = 0; i < 10; ++i) {
        db.get('not found', function (err, value) {
          t.ok(err, 'should error')
          t.ok(verifyNotFoundError(err), 'correct error')
          t.ok(typeof value === 'undefined', 'value is undefined')
          done()
        })
      }
    })
  })

  test('test get() not found error is asynchronous', function (t) {
    t.plan(4)

    let async = false

    db.get('not found', function (err, value) {
      t.ok(err, 'should error')
      t.ok(verifyNotFoundError(err), 'correct error')
      t.ok(typeof value === 'undefined', 'value is undefined')
      t.ok(async, 'callback is asynchronous')
    })

    async = true
  })
}

exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.args(test, testCommon)
  exports.get(test, testCommon)
  exports.tearDown(test, testCommon)
}
