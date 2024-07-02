'use strict'

const { assertAsync, illegalKeys } = require('./util')

let db

/**
 * @param {import('tape')} test
 */
exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

/**
 * @param {import('tape')} test
 */
exports.args = function (test, testCommon) {
  test('test getMany() requires an array argument (callback)', assertAsync.ctx(function (t) {
    // Add 1 assertion for every assertAsync()
    t.plan(6)

    db.getMany('foo', assertAsync(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'keys' must be an array")
    }))
    db.getMany('foo', {}, assertAsync(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'keys' must be an array")
    }))
  }))

  test('test getMany() requires an array argument (promise)', function (t) {
    t.plan(6)

    db.getMany().catch(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'keys' must be an array")
    })
    db.getMany('foo').catch(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'keys' must be an array")
    })
    db.getMany('foo', {}).catch(function (err) {
      t.is(err.name, 'TypeError')
      t.is(err && err.message, "The first argument 'keys' must be an array")
    })
  })

  test('test getMany() with illegal keys', assertAsync.ctx(function (t) {
    // Add 1 assertion for every assertAsync()
    t.plan(illegalKeys.length * 10)

    for (const { name, key } of illegalKeys) {
      db.getMany([key], assertAsync(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (callback)')
        t.is(err && err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (callback)')
      }))

      db.getMany(['valid', key], assertAsync(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (callback, second key)')
        t.is(err && err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (callback, second key)')
      }))

      db.getMany([key]).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise)')
        t.is(err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (promise)')
      })

      db.getMany(['valid', key]).catch(function (err) {
        t.ok(err instanceof Error, name + ' - is Error (promise, second key)')
        t.is(err.code, 'LEVEL_INVALID_KEY', name + ' - correct error code (promise, second key)')
      })
    }
  }))
}

/**
 * @param {import('tape')} test
 */
exports.getMany = function (test, testCommon) {
  test('test simple getMany()', function (t) {
    db.put('foo', 'bar', function (err) {
      t.error(err)

      function verify (err, values) {
        t.error(err)
        t.ok(Array.isArray(values), 'got an array')
        t.is(values.length, 1, 'array has 1 element')
        t.is(values[0], 'bar')
      }

      db.getMany(['foo'], function (err, values) {
        verify(err, values)

        db.getMany(['foo'], {}, function (err, values) {
          verify(err, values)

          db.getMany(['foo'], { valueEncoding: 'utf8' }, function (err, values) {
            t.error(err)
            t.is(values && typeof values[0], 'string', 'should be string if not buffer')
            t.same(values, ['bar'])
            t.end()
          })
        })
      })
    })
  })

  test('test getMany() with multiple keys', function (t) {
    t.plan(5)

    db.put('beep', 'boop', function (err) {
      t.ifError(err)

      db.getMany(['foo', 'beep'], { valueEncoding: 'utf8' }, function (err, values) {
        t.ifError(err)
        t.same(values, ['bar', 'boop'])
      })

      db.getMany(['beep', 'foo'], { valueEncoding: 'utf8' }, function (err, values) {
        t.ifError(err)
        t.same(values, ['boop', 'bar'], 'maintains order of input keys')
      })
    })
  })

  test('test empty getMany()', assertAsync.ctx(function (t) {
    const encodings = Object.keys(db.supports.encodings).filter(k => db.supports.encodings[k])
    t.plan(encodings.length * 3)

    for (const valueEncoding of encodings) {
      db.getMany([], { valueEncoding }, assertAsync(function (err, values) {
        t.ifError(err)
        t.same(values, [])
      }))
    }
  }))

  test('test not-found getMany()', assertAsync.ctx(function (t) {
    const encodings = Object.keys(db.supports.encodings).filter(k => db.supports.encodings[k])
    t.plan(encodings.length * 3)

    for (const valueEncoding of encodings) {
      db.getMany(['nope', 'another'], { valueEncoding }, assertAsync(function (err, values) {
        t.ifError(err)
        t.same(values, [undefined, undefined])
      }))
    }
  }))

  test('test getMany() with promise', async function (t) {
    t.same(await db.getMany(['foo'], { valueEncoding: 'utf8' }), ['bar'])
    t.same(await db.getMany(['beep'], { valueEncoding: 'utf8' }), ['boop'])
    t.same(await db.getMany(['foo', 'beep'], { valueEncoding: 'utf8' }), ['bar', 'boop'])
    t.same(await db.getMany(['beep', 'foo'], { valueEncoding: 'utf8' }), ['boop', 'bar'])
    t.same(await db.getMany(['beep', 'foo', 'nope'], { valueEncoding: 'utf8' }), ['boop', 'bar', undefined])
    t.same(await db.getMany([], { valueEncoding: 'utf8' }), [])
  })

  test('test simultaneous getMany()', function (t) {
    db.put('hello', 'world', function (err) {
      t.error(err)

      let completed = 0
      const done = function () {
        if (++completed === 20) t.end()
      }

      for (let i = 0; i < 10; ++i) {
        db.getMany(['hello'], function (err, values) {
          t.error(err)
          t.is(values.length, 1)
          t.is(values[0] && values[0].toString(), 'world')
          done()
        })
      }

      for (let i = 0; i < 10; ++i) {
        db.getMany(['not found'], function (err, values) {
          t.error(err)
          t.same(values, [undefined])
          done()
        })
      }
    })
  })

  test('test getMany() on opening db', assertAsync.ctx(function (t) {
    t.plan(2 * 2 * 5)

    // Also test empty array because it has a fast-path
    for (const keys of [['foo'], []]) {
      // Opening should make no difference, because we call it after getMany()
      for (const open of [true, false]) {
        const db = testCommon.factory()

        t.is(db.status, 'opening')

        db.getMany(keys, assertAsync(function (err, values) {
          t.ifError(err, 'no error')
          t.same(values, keys.map(_ => undefined))
        }))

        if (open) {
          db.open(t.error.bind(t))
        } else {
          t.pass()
        }
      }
    }
  }))

  test('test getMany() on closed db', function (t) {
    t.plan(2 * 4)

    // Also test empty array because it has a fast-path
    for (const keys of [['foo'], []]) {
      const db = testCommon.factory()

      db.open(function (err) {
        t.ifError(err)

        db.close(assertAsync.with(t, function (err) {
          t.ifError(err)

          db.getMany(keys, assertAsync(function (err) {
            t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
          }))
        }))
      })
    }
  })

  test('test getMany() on closing db', function (t) {
    t.plan(2 * 4)

    // Also test empty array because it has a fast-path
    for (const keys of [['foo'], []]) {
      const db = testCommon.factory()

      db.open(assertAsync.with(t, function (err) {
        t.ifError(err)

        db.close(function (err) {
          t.ifError(err)
        })

        db.getMany(keys, assertAsync(function (err) {
          t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
        }))
      }))
    }
  })
}

/**
 * @param {import('tape')} test
 */
exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

/**
 * @param {import('tape')} test
 */
exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.args(test, testCommon)
  exports.getMany(test, testCommon)
  exports.tearDown(test, testCommon)
}
