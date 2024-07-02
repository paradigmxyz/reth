'use strict'

const { DeferredIterator } = require('../lib/deferred-iterator')

exports.all = function (test, testCommon) {
  function verifyValues (t, db, entries) {
    let pendingGets = 3

    for (let k = 1; k <= entries; k++) {
      db.get('k' + k, { valueEncoding: 'utf8' }, function (err, v) {
        t.ifError(err, 'no get() error')
        t.is(v, 'v' + k, 'value is ok')
        t.is(db.status, 'open', 'status is ok')

        if (--pendingGets <= 0) {
          db.get('k4', { valueEncoding: 'utf8' }, function (err) {
            t.ok(err)
            db.close(t.ifError.bind(t))
          })
        }
      })
    }
  }

  // NOTE: copied from levelup
  test('deferred open(): put() and get() on new database', function (t) {
    t.plan(15)

    // Open database without callback, opens in next tick
    const db = testCommon.factory()

    let pendingPuts = 3

    // Insert 3 values with put(), these should be deferred until the database is actually open
    for (let k = 1; k <= 3; k++) {
      db.put('k' + k, 'v' + k, function (err) {
        t.ifError(err, 'no put() error')

        if (--pendingPuts <= 0) {
          verifyValues(t, db, 3)
        }
      })
    }

    t.is(db.status, 'opening')
  })

  // NOTE: copied from levelup
  test('deferred open(): batch() on new database', function (t) {
    t.plan(13)

    // Open database without callback, opens in next tick
    const db = testCommon.factory()

    // Insert 3 values with batch(), these should be deferred until the database is actually open
    db.batch([
      { type: 'put', key: 'k1', value: 'v1' },
      { type: 'put', key: 'k2', value: 'v2' },
      { type: 'put', key: 'k3', value: 'v3' }
    ], function (err) {
      t.ifError(err, 'no batch() error')
      verifyValues(t, db, 3)
    })

    t.is(db.status, 'opening')
  })

  // NOTE: copied from levelup
  test('deferred open(): chained batch() on new database', function (t) {
    t.plan(13)

    // Open database without callback, opens in next tick
    const db = testCommon.factory()

    // Insert 3 values with batch(), these should be deferred until the database is actually open
    db.batch()
      .put('k1', 'v1')
      .put('k2', 'v2')
      .put('k3', 'v3')
      .write(function (err) {
        t.ifError(err, 'no write() error')
        verifyValues(t, db, 3)
      })

    t.is(db.status, 'opening')
  })

  // NOTE: copied from levelup
  test('deferred open(): put() and get() on reopened database', async function (t) {
    const db = testCommon.factory()

    await db.close()
    t.is(db.status, 'closed')

    db.open(() => {})
    t.is(db.status, 'opening')

    await db.put('beep', 'boop')

    t.is(db.status, 'open')
    t.is(await db.get('beep', { valueEncoding: 'utf8' }), 'boop')

    await db.close()
  })

  // NOTE: copied from levelup
  test('deferred open(): value of queued operation is not stringified', function (t) {
    t.plan(4)

    const db = testCommon.factory({ valueEncoding: 'json' })

    db.put('key', { thing: 2 }, function (err) {
      t.ifError(err)

      db.get('key', function (err, value) {
        t.ifError(err)
        t.same(value, { thing: 2 })
        db.close(t.ifError.bind(t))
      })
    })
  })

  // NOTE: copied from levelup
  test('deferred open(): key of queued operation is not stringified', function (t) {
    t.plan(4)

    const db = testCommon.factory({ keyEncoding: 'json' })

    db.put({ thing: 2 }, 'value', function (err) {
      t.ifError(err)

      db.iterator().next(function (err, key, value) {
        t.ifError(err, 'no next() error')
        t.same(key, { thing: 2 })
        db.close(t.ifError.bind(t))
      })
    })
  })

  // NOTE: copied from deferred-leveldown
  test('cannot operate on closed db', function (t) {
    t.plan(6)

    const db = testCommon.factory()

    db.open(function (err) {
      t.ifError(err)

      db.close(function (err) {
        t.ifError(err)

        db.put('foo', 'bar', function (err) {
          t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
        })

        try {
          db.iterator()
        } catch (err) {
          t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
        }

        try {
          db.keys()
        } catch (err) {
          t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
        }

        try {
          db.values()
        } catch (err) {
          t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
        }
      })
    })
  })

  // NOTE: copied from deferred-leveldown
  test('cannot operate on closing db', function (t) {
    t.plan(6)

    const db = testCommon.factory()

    db.open(function (err) {
      t.ifError(err)

      db.close(function (err) {
        t.ifError(err)
      })

      db.put('foo', 'bar', function (err) {
        t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
      })

      try {
        db.iterator()
      } catch (err) {
        t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
      }

      try {
        db.keys()
      } catch (err) {
        t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
      }

      try {
        db.values()
      } catch (err) {
        t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
      }
    })
  })

  // NOTE: copied from deferred-leveldown
  test('deferred iterator - cannot operate on closed db', function (t) {
    t.plan(10)

    const db = testCommon.factory()

    db.open(function (err) {
      t.error(err, 'no error')

      db.close(function (err) {
        t.ifError(err)

        it.next(function (err, key, value) {
          t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        it.next().catch(function (err) {
          t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        it.nextv(10, function (err, items) {
          t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        it.nextv(10).catch(function (err) {
          t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        it.all(function (err, items) {
          t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        it.all().catch(function (err) {
          t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
        })

        // Was already closed
        it.close(function () {
          t.ifError(err, 'no close() error')
        })

        it.close().catch(function () {
          t.fail('no close() error')
        })

        try {
          it.seek('foo')
        } catch (err) {
          // Should *not* throw
          t.fail(err)
        }
      })
    })

    const it = db.iterator({ gt: 'foo' })
    t.ok(it instanceof DeferredIterator)
  })

  // NOTE: copied from deferred-leveldown
  test('deferred iterator - cannot operate on closing db', function (t) {
    t.plan(10)

    const db = testCommon.factory()

    db.open(function (err) {
      t.error(err, 'no error')

      db.close(function (err) {
        t.ifError(err)
      })

      it.next(function (err, key, value) {
        t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      it.next().catch(function (err) {
        t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      it.nextv(10, function (err) {
        t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      it.nextv(10).catch(function (err) {
        t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      it.all(function (err) {
        t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      it.all().catch(function (err) {
        t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
      })

      // Is already closing
      it.close(function (err) {
        t.ifError(err, 'no close() error')
      })

      it.close().catch(function () {
        t.fail('no close() error')
      })

      try {
        it.seek('foo')
      } catch (err) {
        // Should *not* throw
        t.fail(err)
      }
    })

    const it = db.iterator({ gt: 'foo' })
    t.ok(it instanceof DeferredIterator)
  })
}
