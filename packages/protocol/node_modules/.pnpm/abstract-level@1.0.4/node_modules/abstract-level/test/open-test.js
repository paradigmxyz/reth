'use strict'

const { assertAsync } = require('./util')

exports.open = function (test, testCommon) {
  test('test database open, no options', function (t) {
    const db = testCommon.factory()

    t.is(db.status, 'opening')

    // default createIfMissing=true, errorIfExists=false
    db.open(function (err) {
      t.error(err)
      t.is(db.status, 'open')

      db.close(function () {
        t.is(db.status, 'closed')
        t.end()
      })
    })

    t.is(db.status, 'opening')
  })

  test('test database open, no options, with promise', function (t) {
    const db = testCommon.factory()

    t.is(db.status, 'opening')

    // default createIfMissing=true, errorIfExists=false
    db.open().then(function () {
      t.is(db.status, 'open')
      db.close(t.end.bind(t))
    }).catch(t.fail.bind(t))

    t.is(db.status, 'opening')
  })

  test('test database open, options and callback', function (t) {
    const db = testCommon.factory()

    // default createIfMissing=true, errorIfExists=false
    db.open({}, function (err) {
      t.error(err)
      db.close(function () {
        t.end()
      })
    })
  })

  test('test database open, options with promise', function (t) {
    const db = testCommon.factory()

    // default createIfMissing=true, errorIfExists=false
    db.open({}).then(function () {
      db.close(t.end.bind(t))
    })
  })

  test('test database open, close and open', function (t) {
    const db = testCommon.factory()

    db.open(function (err) {
      t.error(err)

      db.close(function (err) {
        t.error(err)
        t.is(db.status, 'closed')

        db.open(function (err) {
          t.error(err)
          t.is(db.status, 'open')

          db.close(t.end.bind(t))
        })
      })
    })
  })

  test('test database open, close and open with promise', function (t) {
    const db = testCommon.factory()

    db.open().then(function () {
      db.close(function (err) {
        t.error(err)
        db.open().then(function () {
          db.close(function () {
            t.end()
          })
        }).catch(t.fail.bind(t))
      })
    }).catch(t.fail.bind(t))
  })

  test('test database open and close in same tick', assertAsync.ctx(function (t) {
    t.plan(10)

    const db = testCommon.factory()
    const order = []

    db.open(assertAsync(function (err) {
      order.push('A')
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN', 'got open() error')
      t.is(db.status, 'closed', 'is closed')
    }))

    t.is(db.status, 'opening', 'is opening')

    // This wins from the open() call
    db.close(assertAsync(function (err) {
      order.push('B')
      t.same(order, ['A', 'closed event', 'B'], 'order is correct')
      t.ifError(err, 'no close() error')
      t.is(db.status, 'closed', 'is closed')
    }))

    // But open() is still in control
    t.is(db.status, 'opening', 'is still opening')

    // Should not emit 'open', because close() wins
    db.on('open', t.fail.bind(t))
    db.on('closed', assertAsync(() => { order.push('closed event') }))
  }))

  test('test database open, close and open in same tick', assertAsync.ctx(function (t) {
    t.plan(14)

    const db = testCommon.factory()
    const order = []

    db.open(assertAsync(function (err) {
      order.push('A')
      t.ifError(err, 'no open() error (1)')
      t.is(db.status, 'open', 'is open')
    }))

    t.is(db.status, 'opening', 'is opening')

    // This wins from the open() call
    db.close(assertAsync(function (err) {
      order.push('B')
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_CLOSED')
      t.is(db.status, 'open', 'is open')
    }))

    t.is(db.status, 'opening', 'is still opening')

    // This wins from the close() call
    db.open(assertAsync(function (err) {
      order.push('C')
      t.same(order, ['A', 'B', 'open event', 'C'], 'callback order is the same as call order')
      t.ifError(err, 'no open() error (2)')
      t.is(db.status, 'open', 'is open')
    }))

    // Should not emit 'closed', because open() wins
    db.on('closed', t.fail.bind(t))
    db.on('open', assertAsync(() => { order.push('open event') }))

    t.is(db.status, 'opening', 'is still opening')
  }))

  test('test database open if already open (sequential)', function (t) {
    t.plan(7)

    const db = testCommon.factory()

    db.open(assertAsync(function (err) {
      t.ifError(err, 'no open() error (1)')
      t.is(db.status, 'open', 'is open')

      db.open(assertAsync(function (err) {
        t.ifError(err, 'no open() error (2)')
        t.is(db.status, 'open', 'is open')
      }))

      t.is(db.status, 'open', 'not reopening')
      db.on('open', t.fail.bind(t))
      assertAsync.end(t)
    }))

    assertAsync.end(t)
  })

  test('test database open if already opening (parallel)', assertAsync.ctx(function (t) {
    t.plan(7)

    const db = testCommon.factory()

    db.open(assertAsync(function (err) {
      t.ifError(err, 'no open() error (1)')
      t.is(db.status, 'open')
    }))

    db.open(assertAsync(function (err) {
      t.ifError(err, 'no open() error (2)')
      t.is(db.status, 'open')
      db.close(t.end.bind(t))
    }))

    t.is(db.status, 'opening')
  }))

  test('test database close if already closed', function (t) {
    t.plan(8)

    const db = testCommon.factory()

    db.open(function (err) {
      t.ifError(err, 'no open() error')

      db.close(assertAsync(function (err) {
        t.ifError(err, 'no close() error (1)')
        t.is(db.status, 'closed', 'is closed')

        db.close(assertAsync(function (err) {
          t.ifError(err, 'no close() error (2)')
          t.is(db.status, 'closed', 'is closed')
        }))

        t.is(db.status, 'closed', 'is closed', 'not reclosing')
        db.on('closed', t.fail.bind(t))
        assertAsync.end(t)
      }))

      assertAsync.end(t)
    })
  })

  test('test database close if new', assertAsync.ctx(function (t) {
    t.plan(5)

    const db = testCommon.factory()
    const expectedStatus = db.supports.deferredOpen ? 'opening' : 'closed'

    t.is(db.status, expectedStatus, 'status ok')

    db.close(assertAsync(function (err) {
      t.ifError(err, 'no close() error')
      t.is(db.status, 'closed', 'status ok')
    }))

    t.is(db.status, expectedStatus, 'status unchanged')

    if (!db.supports.deferredOpen) {
      db.on('closed', t.fail.bind(t, 'should not emit closed'))
    }
  }))

  test('test database close on open event', function (t) {
    t.plan(5)

    const db = testCommon.factory()
    const order = []

    db.open(function (err) {
      order.push('A')
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN', 'got open() error')
      t.is(db.status, 'closed', 'is closed')
    })

    db.on('open', function () {
      // This wins from the (still in progress) open() call
      db.close(function (err) {
        order.push('B')
        t.same(order, ['A', 'closed event', 'B'], 'order is correct')
        t.ifError(err, 'no close() error')
        t.is(db.status, 'closed', 'is closed')
      })
    })

    db.on('closed', () => { order.push('closed event') })
  })

  test('test passive open()', async function (t) {
    t.plan(1)
    const db = testCommon.factory()
    await db.open({ passive: true }) // OK, already opening
    await db.close()
    await db.open({ passive: true }).catch(err => {
      t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
    })
    await db.open()
    await db.open({ passive: true }) // OK, already open
    return db.close()
  })

  test('test passive open(): ignored if set in constructor options', async function (t) {
    const db = testCommon.factory({ passive: true })
    await new Promise((resolve) => db.once('open', resolve))
    return db.close()
  })
}

exports.all = function (test, testCommon) {
  exports.open(test, testCommon)
}
