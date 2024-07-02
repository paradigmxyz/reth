'use strict'

const { Buffer } = require('buffer')
const identity = (v) => v

let db

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

exports.args = function (test, testCommon) {
  for (const mode of ['iterator', 'keys', 'values']) {
    test(`${mode}() has db reference`, async function (t) {
      const it = db[mode]()

      // May return iterator of an underlying db, that's okay.
      t.ok(it.db === db || it.db === (db.db || db._db || db))

      await it.close()
    })

    test(`${mode}() has limit and count properties`, async function (t) {
      const iterators = [db[mode]()]
      t.is(iterators[0].limit, Infinity, 'defaults to infinite')

      for (const limit of [-1, 0, 1, Infinity]) {
        const it = db[mode]({ limit })
        iterators.push(it)
        t.is(it.limit, limit === -1 ? Infinity : limit, 'has limit property')
      }

      t.ok(iterators.every(it => it.count === 0), 'has count property')
      await Promise.all(iterators.map(it => it.close()))
    })

    test(`${mode}().nextv() yields error if size is invalid`, async function (t) {
      t.plan(4)

      const it = db[mode]()

      for (const args of [[], [NaN], ['1'], [2.5]]) {
        try {
          await it.nextv(...args)
        } catch (err) {
          t.is(err.message, "The first argument 'size' must be an integer")
        }
      }

      await it.close()
    })
  }
}

exports.sequence = function (test, testCommon) {
  for (const mode of ['iterator', 'keys', 'values']) {
    test(`${mode}().close() is idempotent`, function (t) {
      const iterator = db[mode]()

      iterator.close(function () {
        let async = false

        iterator.close(function () {
          t.ok(async, 'callback is asynchronous')
          t.end()
        })

        async = true
      })
    })

    for (const method of ['next', 'nextv', 'all']) {
      const requiredArgs = method === 'nextv' ? [1] : []

      test(`${mode}().${method}() after close() yields error`, function (t) {
        const iterator = db[mode]()
        iterator.close(function (err) {
          t.error(err)

          let async = false

          iterator[method](...requiredArgs, function (err2) {
            t.ok(err2, 'returned error')
            t.is(err2.code, 'LEVEL_ITERATOR_NOT_OPEN', 'correct message')
            t.ok(async, 'callback is asynchronous')
            t.end()
          })

          async = true
        })
      })

      for (const otherMethod of ['next', 'nextv', 'all']) {
        const otherRequiredArgs = otherMethod === 'nextv' ? [1] : []

        test(`${mode}().${method}() while busy with ${otherMethod}() yields error`, function (t) {
          const iterator = db[mode]()
          iterator[otherMethod](...otherRequiredArgs, function (err) {
            t.error(err)
            iterator.close(function (err) {
              t.error(err)
              t.end()
            })
          })

          let async = false

          iterator[method](...requiredArgs, function (err) {
            t.ok(err, 'returned error')
            t.is(err.code, 'LEVEL_ITERATOR_BUSY')
            t.ok(async, 'callback is asynchronous')
          })

          async = true
        })
      }
    }
  }

  for (const deferred of [false, true]) {
    for (const mode of ['iterator', 'keys', 'values']) {
      for (const method of ['next', 'nextv', 'all']) {
        const requiredArgs = method === 'nextv' ? [10] : []

        // NOTE: adapted from leveldown
        test(`${mode}().${method}() after db.close() yields error (deferred: ${deferred})`, async function (t) {
          t.plan(2)

          const db = testCommon.factory()
          if (!deferred) await db.open()

          await db.put('a', 'a')
          await db.put('b', 'b')

          const it = db[mode]()

          // The first call *should* succeed, because it was scheduled before close(). However, success
          // is not a must. Because nextv() and all() fallback to next*(), they're allowed to fail. An
          // implementation can also choose to abort any pending call on close.
          let promise = it[method](...requiredArgs).then(() => {
            t.pass('Optionally succeeded')
          }).catch((err) => {
            t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
          })

          // The second call *must* fail, because it was scheduled after close()
          promise = promise.then(() => {
            return it[method](...requiredArgs).then(() => {
              t.fail('Expected an error')
            }).catch((err) => {
              t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
            })
          })

          return Promise.all([db.close(), promise])
        })
      }
    }
  }
}

exports.iterator = function (test, testCommon) {
  test('test simple iterator()', function (t) {
    const data = [
      { type: 'put', key: 'foobatch1', value: 'bar1' },
      { type: 'put', key: 'foobatch2', value: 'bar2' },
      { type: 'put', key: 'foobatch3', value: 'bar3' }
    ]
    let idx = 0

    db.batch(data, function (err) {
      t.error(err)
      const iterator = db.iterator()
      const fn = function (err, key, value) {
        t.error(err)
        if (key && value) {
          t.is(key, data[idx].key, 'correct key')
          t.is(value, data[idx].value, 'correct value')
          db.nextTick(next)
          idx++
        } else { // end
          t.ok(err == null, 'err argument is nullish')
          t.ok(typeof key === 'undefined', 'key argument is undefined')
          t.ok(typeof value === 'undefined', 'value argument is undefined')
          t.is(idx, data.length, 'correct number of entries')
          iterator.close(function () {
            t.end()
          })
        }
      }
      const next = function () {
        iterator.next(fn)
      }

      next()
    })
  })

  // NOTE: adapted from leveldown
  test('key-only iterator', function (t) {
    const it = db.iterator({ values: false })

    it.next(function (err, key, value) {
      t.ifError(err, 'no next() error')
      t.is(key, 'foobatch1')
      t.is(value, undefined)
      it.close(t.end.bind(t))
    })
  })

  // NOTE: adapted from leveldown
  test('value-only iterator', function (t) {
    const it = db.iterator({ keys: false })

    it.next(function (err, key, value) {
      t.ifError(err, 'no next() error')
      t.is(key, undefined)
      t.is(value, 'bar1')
      it.close(t.end.bind(t))
    })
  })

  test('db.keys().next()', function (t) {
    const it = db.keys()

    it.next(function (err, key) {
      t.ifError(err, 'no next() error')
      t.is(key, 'foobatch1')
      it.close(t.end.bind(t))
    })
  })

  test('db.values().next()', function (t) {
    const it = db.values()

    it.next(function (err, value) {
      t.ifError(err, 'no next() error')
      t.is(value, 'bar1')
      it.close(t.end.bind(t))
    })
  })

  for (const mode of ['iterator', 'keys', 'values']) {
    const mapEntry = e => mode === 'iterator' ? e : mode === 'keys' ? e[0] : e[1]

    test(`${mode}().nextv()`, async function (t) {
      const it = db[mode]()

      t.same(await it.nextv(1), [['foobatch1', 'bar1']].map(mapEntry))
      t.same(await it.nextv(2, {}), [['foobatch2', 'bar2'], ['foobatch3', 'bar3']].map(mapEntry))
      t.same(await it.nextv(2), [])

      await it.close()
    })

    test(`${mode}().nextv() in reverse`, async function (t) {
      const it = db[mode]({ reverse: true })

      t.same(await it.nextv(1), [['foobatch3', 'bar3']].map(mapEntry))
      t.same(await it.nextv(2, {}), [['foobatch2', 'bar2'], ['foobatch1', 'bar1']].map(mapEntry))
      t.same(await it.nextv(2), [])

      await it.close()
    })

    test(`${mode}().nextv() has soft minimum of 1`, async function (t) {
      const it = db[mode]()

      t.same(await it.nextv(0), [['foobatch1', 'bar1']].map(mapEntry))
      t.same(await it.nextv(0), [['foobatch2', 'bar2']].map(mapEntry))
      t.same(await it.nextv(0, {}), [['foobatch3', 'bar3']].map(mapEntry))
      t.same(await it.nextv(0), [])

      await it.close()
    })

    test(`${mode}().nextv() requesting more than available`, async function (t) {
      const it = db[mode]()

      t.same(await it.nextv(10), [
        ['foobatch1', 'bar1'],
        ['foobatch2', 'bar2'],
        ['foobatch3', 'bar3']
      ].map(mapEntry))
      t.same(await it.nextv(10), [])

      await it.close()
    })

    test(`${mode}().nextv() honors limit`, async function (t) {
      const it = db[mode]({ limit: 2 })

      t.same(await it.nextv(10), [['foobatch1', 'bar1'], ['foobatch2', 'bar2']].map(mapEntry))
      t.same(await it.nextv(10), [])

      await it.close()
    })

    test(`${mode}().nextv() honors limit in reverse`, async function (t) {
      const it = db[mode]({ limit: 2, reverse: true })

      t.same(await it.nextv(10), [['foobatch3', 'bar3'], ['foobatch2', 'bar2']].map(mapEntry))
      t.same(await it.nextv(10), [])

      await it.close()
    })

    test(`${mode}().all()`, async function (t) {
      t.same(await db[mode]().all(), [
        ['foobatch1', 'bar1'],
        ['foobatch2', 'bar2'],
        ['foobatch3', 'bar3']
      ].map(mapEntry))

      t.same(await db[mode]().all({}), [
        ['foobatch1', 'bar1'],
        ['foobatch2', 'bar2'],
        ['foobatch3', 'bar3']
      ].map(mapEntry))
    })

    test(`${mode}().all() in reverse`, async function (t) {
      t.same(await db[mode]({ reverse: true }).all(), [
        ['foobatch3', 'bar3'],
        ['foobatch2', 'bar2'],
        ['foobatch1', 'bar1']
      ].map(mapEntry))
    })

    test(`${mode}().all() honors limit`, async function (t) {
      t.same(await db[mode]({ limit: 2 }).all(), [
        ['foobatch1', 'bar1'],
        ['foobatch2', 'bar2']
      ].map(mapEntry))

      const it = db[mode]({ limit: 2 })

      t.same(await it.next(), mapEntry(['foobatch1', 'bar1']))
      t.same(await it.all(), [['foobatch2', 'bar2']].map(mapEntry))
    })

    test(`${mode}().all() honors limit in reverse`, async function (t) {
      t.same(await db[mode]({ limit: 2, reverse: true }).all(), [
        ['foobatch3', 'bar3'],
        ['foobatch2', 'bar2']
      ].map(mapEntry))

      const it = db[mode]({ limit: 2, reverse: true })

      t.same(await it.next(), mapEntry(['foobatch3', 'bar3']))
      t.same(await it.all(), [['foobatch2', 'bar2']].map(mapEntry))
    })
  }

  // NOTE: adapted from memdown
  test('iterator() sorts lexicographically', async function (t) {
    const db = testCommon.factory()
    await db.open()

    // Write in unsorted order with multiple operations
    await db.put('f', 'F')
    await db.put('a', 'A')
    await db.put('~', '~')
    await db.put('e', 'E')
    await db.put('🐄', '🐄')
    await db.batch([
      { type: 'put', key: 'd', value: 'D' },
      { type: 'put', key: 'b', value: 'B' },
      { type: 'put', key: 'ff', value: 'FF' },
      { type: 'put', key: 'a🐄', value: 'A🐄' }
    ])
    await db.batch([
      { type: 'put', key: '', value: 'empty' },
      { type: 'put', key: '2', value: '2' },
      { type: 'put', key: '12', value: '12' },
      { type: 'put', key: '\t', value: '\t' }
    ])

    t.same(await db.iterator().all(), [
      ['', 'empty'],
      ['\t', '\t'],
      ['12', '12'],
      ['2', '2'],
      ['a', 'A'],
      ['a🐄', 'A🐄'],
      ['b', 'B'],
      ['d', 'D'],
      ['e', 'E'],
      ['f', 'F'],
      ['ff', 'FF'],
      ['~', '~'],
      ['🐄', '🐄']
    ])

    t.same(await db.iterator({ lte: '' }).all(), [
      ['', 'empty']
    ])

    return db.close()
  })

  for (const keyEncoding of ['buffer', 'view']) {
    if (!testCommon.supports.encodings[keyEncoding]) continue

    test(`test iterator() has byte order (${keyEncoding} encoding)`, function (t) {
      const db = testCommon.factory({ keyEncoding })

      db.open(function (err) {
        t.ifError(err, 'no open() error')

        const ctor = keyEncoding === 'buffer' ? Buffer : Uint8Array
        const keys = [2, 11, 1].map(b => ctor.from([b]))

        db.batch(keys.map((key) => ({ type: 'put', key, value: 'x' })), function (err) {
          t.ifError(err, 'no batch() error')

          db.keys().all(function (err, keys) {
            t.ifError(err, 'no all() error')
            t.same(keys.map(k => k[0]), [1, 2, 11], 'order is ok')

            db.iterator().all(function (err, entries) {
              t.ifError(err, 'no all() error')
              t.same(entries.map(e => e[0][0]), [1, 2, 11], 'order is ok')

              db.close(t.end.bind(t))
            })
          })
        })
      })
    })

    // NOTE: adapted from memdown and level-js
    test(`test iterator() with byte range (${keyEncoding} encoding)`, async function (t) {
      const db = testCommon.factory({ keyEncoding })
      await db.open()

      await db.put(Uint8Array.from([0x0]), '0')
      await db.put(Uint8Array.from([128]), '128')
      await db.put(Uint8Array.from([160]), '160')
      await db.put(Uint8Array.from([192]), '192')

      const collect = async (range) => {
        const entries = await db.iterator(range).all()
        t.ok(entries.every(e => e[0] instanceof Uint8Array)) // True for both encodings
        t.ok(entries.every(e => e[1] === String(e[0][0])))
        return entries.map(e => e[0][0])
      }

      t.same(await collect({ gt: Uint8Array.from([255]) }), [])
      t.same(await collect({ gt: Uint8Array.from([192]) }), [])
      t.same(await collect({ gt: Uint8Array.from([160]) }), [192])
      t.same(await collect({ gt: Uint8Array.from([128]) }), [160, 192])
      t.same(await collect({ gt: Uint8Array.from([0x0]) }), [128, 160, 192])
      t.same(await collect({ gt: Uint8Array.from([]) }), [0x0, 128, 160, 192])

      t.same(await collect({ lt: Uint8Array.from([255]) }), [0x0, 128, 160, 192])
      t.same(await collect({ lt: Uint8Array.from([192]) }), [0x0, 128, 160])
      t.same(await collect({ lt: Uint8Array.from([160]) }), [0x0, 128])
      t.same(await collect({ lt: Uint8Array.from([128]) }), [0x0])
      t.same(await collect({ lt: Uint8Array.from([0x0]) }), [])
      t.same(await collect({ lt: Uint8Array.from([]) }), [])

      t.same(await collect({ gte: Uint8Array.from([255]) }), [])
      t.same(await collect({ gte: Uint8Array.from([192]) }), [192])
      t.same(await collect({ gte: Uint8Array.from([160]) }), [160, 192])
      t.same(await collect({ gte: Uint8Array.from([128]) }), [128, 160, 192])
      t.same(await collect({ gte: Uint8Array.from([0x0]) }), [0x0, 128, 160, 192])
      t.same(await collect({ gte: Uint8Array.from([]) }), [0x0, 128, 160, 192])

      t.same(await collect({ lte: Uint8Array.from([255]) }), [0x0, 128, 160, 192])
      t.same(await collect({ lte: Uint8Array.from([192]) }), [0x0, 128, 160, 192])
      t.same(await collect({ lte: Uint8Array.from([160]) }), [0x0, 128, 160])
      t.same(await collect({ lte: Uint8Array.from([128]) }), [0x0, 128])
      t.same(await collect({ lte: Uint8Array.from([0x0]) }), [0x0])
      t.same(await collect({ lte: Uint8Array.from([]) }), [])

      return db.close()
    })
  }
}

exports.decode = function (test, testCommon) {
  for (const deferred of [false, true]) {
    for (const mode of ['iterator', 'keys', 'values']) {
      for (const method of ['next', 'nextv', 'all']) {
        const requiredArgs = method === 'nextv' ? [1] : []

        for (const encodingOption of ['keyEncoding', 'valueEncoding']) {
          if (mode === 'keys' && encodingOption === 'valueEncoding') continue
          if (mode === 'values' && encodingOption === 'keyEncoding') continue

          // NOTE: adapted from encoding-down
          test(`${mode}().${method}() catches decoding error from ${encodingOption} (deferred: ${deferred})`, async function (t) {
            t.plan(4)

            const encoding = {
              format: 'utf8',
              decode: function (x) {
                t.is(x, encodingOption === 'keyEncoding' ? 'testKey' : 'testValue')
                throw new Error('from encoding')
              },
              encode: identity
            }

            const db = testCommon.factory()
            await db.put('testKey', 'testValue')

            if (deferred) {
              await db.close()
              db.open(t.ifError.bind(t))
            } else {
              t.pass('non-deferred')
            }

            const it = db[mode]({ [encodingOption]: encoding })

            try {
              await it[method](...requiredArgs)
            } catch (err) {
              t.is(err.code, 'LEVEL_DECODE_ERROR')
              t.is(err.cause && err.cause.message, 'from encoding')
            }

            return db.close()
          })
        }
      }
    }
  }
}

exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.args(test, testCommon)
  exports.sequence(test, testCommon)
  exports.iterator(test, testCommon)
  exports.decode(test, testCommon)
  exports.tearDown(test, testCommon)
}
