'use strict'

const test = require('tape')
const { mockLevel } = require('../util')
const nextTick = require('../../lib/next-tick')

test('resource must be an object with a close() method', function (t) {
  t.plan(5)

  const db = mockLevel()

  for (const invalid of [null, undefined, {}, { close: 123 }]) {
    try {
      db.attachResource(invalid)
    } catch (err) {
      t.is(err && err.message, 'The first argument must be a resource object')
    }
  }

  db.close(t.ifError.bind(t))
})

test('resource is closed on failed open', function (t) {
  t.plan(2)

  const db = mockLevel({
    _open: function (options, callback) {
      t.pass('opened')
      this.nextTick(callback, new Error('_open error'))
    }
  })

  const resource = {
    close: function (cb) {
      // Note: resource shouldn't care about db.status
      t.pass('closed')
      nextTick(cb)
    }
  }

  db.attachResource(resource)
})

test('resource is closed on db.close()', function (t) {
  t.plan(2)

  const db = mockLevel()

  const resource = {
    close: function (cb) {
      // Note: resource shouldn't care about db.status
      t.pass('closed')
      nextTick(cb)
    }
  }

  db.attachResource(resource)
  db.close(t.ifError.bind(t))
})

test('resource is not closed on db.close() if detached', function (t) {
  t.plan(1)

  const db = mockLevel()

  const resource = {
    close: function (cb) {
      t.fail('should not be called')
    }
  }

  db.attachResource(resource)
  db.detachResource(resource)
  db.close(t.ifError.bind(t))
})
