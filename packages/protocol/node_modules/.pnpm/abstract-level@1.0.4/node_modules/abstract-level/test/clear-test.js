'use strict'

const isBuffer = require('is-buffer')
const { Buffer } = require('buffer')

exports.args = function (test, testCommon) {
  test('test clear() with legacy range options', function (t) {
    t.plan(4)

    const db = testCommon.factory()

    db.open(function (err) {
      t.ifError(err)

      try {
        db.clear({ start: 'foo' }, t.fail.bind(t))
      } catch (err) {
        t.is(err.code, 'LEVEL_LEGACY')
      }

      try {
        db.clear({ end: 'foo' }).catch(t.fail.bind(t))
      } catch (err) {
        t.is(err.code, 'LEVEL_LEGACY')
      }

      db.close(t.ifError.bind(t))
    })
  })
}

exports.clear = function (test, testCommon) {
  makeTest('string', ['a', 'b'])

  if (testCommon.supports.encodings.buffer) {
    makeTest('buffer', [Buffer.from('a'), Buffer.from('b')])
    makeTest('mixed', [Buffer.from('a'), 'b'])

    // These keys would be equal when compared as utf8 strings
    makeTest('non-utf8 buffer', [Buffer.from('80', 'hex'), Buffer.from('c0', 'hex')])
  }

  function makeTest (type, keys) {
    test('test simple clear() on ' + type + ' keys', function (t) {
      t.plan(8)

      const db = testCommon.factory()
      const ops = keys.map(function (key) {
        return { type: 'put', key: key, value: 'foo', keyEncoding: isBuffer(key) ? 'buffer' : 'utf8' }
      })

      db.open(function (err) {
        t.ifError(err, 'no open error')

        db.batch(ops, function (err) {
          t.ifError(err, 'no batch error')

          db.iterator().all(function (err, entries) {
            t.ifError(err, 'no all() error')
            t.is(entries.length, keys.length, 'has entries')

            db.clear(function (err) {
              t.ifError(err, 'no clear error')

              db.iterator().all(function (err, entries) {
                t.ifError(err, 'no all() error')
                t.is(entries.length, 0, 'has no entries')

                db.close(function (err) {
                  t.ifError(err, 'no close error')
                })
              })
            })
          })
        })
      })
    })

    test('test simple clear() on ' + type + ' keys, with promise', function (t) {
      t.plan(8)

      const db = testCommon.factory()
      const ops = keys.map(function (key) {
        return { type: 'put', key: key, value: 'foo', keyEncoding: isBuffer(key) ? 'buffer' : 'utf8' }
      })

      db.open(function (err) {
        t.ifError(err, 'no open error')

        db.batch(ops, function (err) {
          t.ifError(err, 'no batch error')

          db.iterator().all(function (err, entries) {
            t.ifError(err, 'no all() error')
            t.is(entries.length, keys.length, 'has entries')

            db.clear().then(function () {
              t.ifError(err, 'no clear error')

              db.iterator().all(function (err, entries) {
                t.ifError(err, 'no all() error')
                t.is(entries.length, 0, 'has no entries')

                db.close(function (err) {
                  t.ifError(err, 'no close error')
                })
              })
            }).catch(t.fail.bind(t))
          })
        })
      })
    })
  }

  // NOTE: adapted from levelup
  for (const deferred of [false, true]) {
    for (const [gte, keyEncoding] of [['"b"', 'utf8'], ['b', 'json']]) {
      test(`clear() with ${keyEncoding} encoding (deferred: ${deferred})`, async function (t) {
        const db = testCommon.factory()

        await db.open()
        await db.batch([
          { type: 'put', key: '"a"', value: 'a' },
          { type: 'put', key: '"b"', value: 'b' }
        ])

        if (deferred) {
          await db.close()
          t.is(db.status, 'closed')
          db.open(t.ifError.bind(t))
          t.is(db.status, 'opening')
        }

        await db.clear({ gte, keyEncoding })

        const keys = await db.keys().all()
        t.same(keys, ['"a"'], 'got expected keys')

        return db.close()
      })
    }
  }
}

exports.events = function (test, testCommon) {
  test('test clear() with options emits clear event', async function (t) {
    t.plan(2)

    const db = testCommon.factory()
    await db.open()

    t.ok(db.supports.events.clear)

    db.on('clear', function (options) {
      t.same(options, { gt: 567, custom: 123 })
    })

    await db.clear({ gt: 567, custom: 123 })
    await db.close()
  })

  test('test clear() without options emits clear event', async function (t) {
    t.plan(2)

    const db = testCommon.factory()
    await db.open()

    t.ok(db.supports.events.clear)

    db.on('clear', function (options) {
      t.same(options, {})
    })

    await db.clear()
    await db.close()
  })
}

exports.all = function (test, testCommon) {
  exports.args(test, testCommon)
  exports.events(test, testCommon)
  exports.clear(test, testCommon)
}
