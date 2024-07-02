'use strict'

const test = require('tape')
const { mockLevel } = require('../util')
const { DefaultChainedBatch } = require('../../lib/default-chained-batch')
const identity = (v) => v

// NOTE: adapted from deferred-leveldown
test('deferred chained batch encodes once', function (t) {
  t.plan(9)

  let called = false

  const keyEncoding = {
    format: 'utf8',
    encode (key) {
      t.is(called, false, 'not yet called')
      t.is(key, 'foo')
      return key.toUpperCase()
    },
    decode: identity
  }

  const valueEncoding = {
    format: 'utf8',
    encode (value) {
      t.is(called, false, 'not yet called')
      t.is(value, 'bar')
      return value.toUpperCase()
    },
    decode: identity
  }

  const db = mockLevel({
    _batch: function (array, options, callback) {
      called = true
      t.is(array[0] && array[0].key, 'FOO')
      t.is(array[0] && array[0].value, 'BAR')
      this.nextTick(callback)
    },
    _open: function (options, callback) {
      t.is(called, false, 'not yet called')
      this.nextTick(callback)
    }
  }, { encodings: { utf8: true } }, {
    keyEncoding,
    valueEncoding
  })

  db.once('open', function () {
    t.is(called, true, 'called')
  })

  db.batch().put('foo', 'bar').write(function (err) {
    t.ifError(err, 'no write() error')
  })
})

test('deferred chained batch is closed upon failed open', function (t) {
  t.plan(6)

  const db = mockLevel({
    _open (options, callback) {
      t.pass('opening')
      this.nextTick(callback, new Error('_open error'))
    },
    _batch () {
      t.fail('should not be called')
    }
  })

  const batch = db.batch()
  t.ok(batch instanceof DefaultChainedBatch)

  batch.put('foo', 'bar')
  batch.del('123')

  batch.write(function (err) {
    t.is(err && err.code, 'LEVEL_BATCH_NOT_OPEN')

    // Should account for userland code that ignores errors
    try {
      batch.put('beep', 'boop')
    } catch (err) {
      t.is(err && err.code, 'LEVEL_BATCH_NOT_OPEN')
    }

    try {
      batch.del('456')
    } catch (err) {
      t.is(err && err.code, 'LEVEL_BATCH_NOT_OPEN')
    }

    batch.write(function (err) {
      t.is(err && err.code, 'LEVEL_BATCH_NOT_OPEN')
    })
  })
})
