'use strict'

const { assertAsync, illegalKeys, illegalValues } = require('./util')

let db

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

exports.args = function (test, testCommon) {
  test('test put() with illegal keys', assertAsync.ctx(function (t) {
    t.plan(illegalKeys.length * 5)

    for (const { name, key } of illegalKeys) {
      db.put(key, 'value', assertAsync(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (callback)')
      }))

      db.put(key, 'value').catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (promise)')
      })
    }
  }))

  test('test put() with illegal values', assertAsync.ctx(function (t) {
    t.plan(illegalValues.length * 5)

    for (const { name, value } of illegalValues) {
      db.put('key', value, assertAsync(function (err) {
        t.ok(err instanceof Error, name + '- is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_VALUE', name + ' - correct error code (callback)')
      }))

      db.put('key', value).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_VALUE', name + ' - correct error code (promise)')
      })
    }
  }))
}

exports.put = function (test, testCommon) {
  test('test simple put()', assertAsync.ctx(function (t) {
    t.plan(7)

    db.put('foo', 'bar', assertAsync(function (err) {
      t.ifError(err, 'no put() error')

      db.get('foo', function (err, value) {
        t.ifError(err, 'no get() error')
        t.is(value, 'bar')

        db.put('foo', 'new', function (err) {
          t.ifError(err, 'no put() error')

          db.get('foo', function (err, value) {
            t.ifError(err, 'no get() error')
            t.is(value, 'new', 'value was overwritten')
          })
        })
      })
    }))
  }))

  test('test simple put() with promise', async function (t) {
    await db.put('foo2', 'bar')
    t.is(await db.get('foo2'), 'bar')
  })

  test('test deferred put()', assertAsync.ctx(function (t) {
    t.plan(5)

    const db = testCommon.factory()

    db.put('foo', 'bar', assertAsync(function (err) {
      t.ifError(err, 'no put() error')

      db.get('foo', { valueEncoding: 'utf8' }, function (err, value) {
        t.ifError(err, 'no get() error')
        t.is(value, 'bar', 'value is ok')
        db.close(t.ifError.bind(t))
      })
    }))
  }))

  test('test deferred put() with promise', async function (t) {
    const db = testCommon.factory()
    await db.put('foo', 'bar')
    t.is(await db.get('foo', { valueEncoding: 'utf8' }), 'bar', 'value is ok')
    return db.close()
  })
}

exports.events = function (test, testCommon) {
  test('test put() emits put event', async function (t) {
    t.plan(3)

    const db = testCommon.factory()
    await db.open()

    t.ok(db.supports.events.put)

    db.on('put', function (key, value) {
      t.is(key, 123)
      t.is(value, 'b')
    })

    await db.put(123, 'b')
    await db.close()
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
  exports.put(test, testCommon)
  exports.events(test, testCommon)
  exports.tearDown(test, testCommon)
}
