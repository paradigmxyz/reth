'use strict'

const test = require('tape')
const { mockLevel } = require('../util')

test('defer() requires valid function argument', function (t) {
  t.plan(7)

  const db = mockLevel()

  for (const invalid of [123, true, false, null, undefined, {}]) {
    try {
      db.defer(invalid)
    } catch (err) {
      t.is(err.message, 'The first argument must be a function')
    }
  }

  db.close(t.ifError.bind(t))
})

test('defer() custom operation', function (t) {
  t.plan(6)

  const db = mockLevel({
    custom (arg, callback) {
      if (this.status === 'opening') {
        t.is(arg, 123)
        this.defer(() => this.custom(456, callback))
      } else {
        t.is(db.status, 'open')
        t.is(arg, 456)
        this.nextTick(callback, null, 987)
      }
    }
  })

  db.custom(123, function (err, result) {
    t.ifError(err, 'no custom() error')
    t.is(result, 987, 'result ok')

    db.close(t.ifError.bind(t))
  })
})

test('defer() custom operation with failed open', function (t) {
  t.plan(4)

  const db = mockLevel({
    _open (options, callback) {
      t.pass('opened')
      this.nextTick(callback, new Error('_open error'))
    },
    custom (arg, callback) {
      if (this.status === 'opening') {
        this.defer(() => this.custom(arg, callback))
      } else {
        t.is(db.status, 'closed')
        this.nextTick(callback, new Error('Database is not open (x)'))
      }
    }
  })

  db.custom(123, function (err, result) {
    t.is(err && err.message, 'Database is not open (x)')
    t.is(result, undefined, 'result ok')
  })
})

test('defer() can drop custom synchronous operation', function (t) {
  t.plan(3)

  const db = mockLevel({
    _open (options, callback) {
      t.pass('opened')
      this.nextTick(callback, new Error('_open error'))
    },
    custom (arg) {
      if (this.status === 'opening') {
        this.defer(() => this.custom(arg * 2))
      } else {
        // Handling other states is a userland responsibility
        t.is(db.status, 'closed')
        t.is(arg, 246)
      }
    }
  })

  db.custom(123)
})
