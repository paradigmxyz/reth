'use strict'

const test = require('tape')
const { Buffer } = require('buffer')
const { AbstractLevel } = require('../..')
const { AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('../..')
const { mockLevel, mockIterator, nullishEncoding } = require('../util')

const identity = (v) => v
const utf8Manifest = { encodings: { utf8: true } }
const dualManifest = { encodings: { utf8: true, buffer: true } }
const tripleManifest = { encodings: { utf8: true, buffer: true, view: true } }

for (const deferred of [false, true]) {
  // Also test default fallback implementations of keys() and values()
  for (const [mode, def] of [['iterator', false], ['keys', false], ['values', false], ['keys', true], ['values', true]]) {
    const Ctor = mode === 'iterator' || def ? AbstractIterator : mode === 'keys' ? AbstractKeyIterator : AbstractValueIterator
    const privateMethod = def ? '_iterator' : '_' + mode
    const publicMethod = mode

    test(`${mode}() (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(4)

      let called = false
      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(this, db, 'thisArg is correct')
          t.is(arguments.length, 1, 'got one argument')

          const kvOptions = mode === 'iterator' || def
            ? { keys: mode !== 'values', values: mode !== 'keys' }
            : {}

          t.same(options, {
            reverse: false,
            limit: -1,
            keyEncoding: 'utf8',
            valueEncoding: 'utf8',
            ...kvOptions
          })

          called = true
          return new Ctor(this, options)
        }
      }

      const db = new MockLevel(tripleManifest)
      if (!deferred) await db.open()

      db[publicMethod]()
      t.is(called, !deferred)
      if (deferred) await db.open()
    })

    test(`${mode}() with custom options (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(3)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.foo, 123)
          t.is(options.reverse, true)
          t.is(options.limit, 1)

          return new Ctor(this, options)
        }
      }

      const db = new MockLevel(tripleManifest)
      if (!deferred) await db.open()
      db[publicMethod]({ foo: 123, reverse: true, limit: 1 })
      if (deferred) await db.open()
    })

    for (const limit of [2, 0]) {
      test(`${mode}().next() skips _next() when limit ${limit} is reached (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
        class MockLevel extends AbstractLevel {
          [privateMethod] (options) {
            return new MockIterator(this, options)
          }
        }

        let calls = 0
        let yielded = 0

        class MockIterator extends Ctor {
          _next (callback) {
            calls++

            if (mode === 'iterator' || def) {
              this.nextTick(callback, null, 'a', 'a')
            } else {
              this.nextTick(callback, null, 'a')
            }
          }
        }

        const db = new MockLevel(utf8Manifest)
        if (!deferred) await db.open()

        const it = db[publicMethod]({ limit })

        for (let i = 0; i < limit + 2; i++) {
          const item = await it.next()
          if (item === undefined) break
          yielded++
        }

        t.is(it.count, limit, 'final count matches limit')
        t.is(calls, limit)
        t.is(yielded, limit)
      })

      test(`${mode}().nextv() skips _nextv() when limit ${limit} is reached (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
        class MockLevel extends AbstractLevel {
          [privateMethod] (options) {
            return new MockIterator(this, options)
          }
        }

        let calls = 0
        let yielded = 0

        class MockIterator extends Ctor {
          _nextv (size, options, callback) {
            calls++

            if (mode === 'iterator' || def) {
              this.nextTick(callback, null, [['a', 'a']])
            } else {
              this.nextTick(callback, null, ['a'])
            }
          }
        }

        const db = new MockLevel(utf8Manifest)
        if (!deferred) await db.open()

        const it = db[publicMethod]({ limit })

        for (let i = 0; i < limit + 2; i++) {
          const items = await it.nextv(1)
          yielded += items.length
          if (items.length === 0) break
        }

        t.is(it.count, limit, 'final count matches limit')
        t.is(calls, limit)
        t.is(yielded, limit)
      })

      test(`${mode}().all() skips _all() when limit ${limit} is reached (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
        class MockLevel extends AbstractLevel {
          [privateMethod] (options) {
            return new MockIterator(this, options)
          }
        }

        let nextCount = 0
        class MockIterator extends Ctor {
          _next (callback) {
            if (++nextCount > 10) {
              throw new Error('Potential infinite loop')
            } else if (mode === 'iterator' || def) {
              this.nextTick(callback, null, 'a', 'a')
            } else {
              this.nextTick(callback, null, 'a')
            }
          }

          _all (options, callback) {
            t.fail('should not be called')
          }
        }

        const db = new MockLevel(utf8Manifest)
        if (!deferred) await db.open()

        const it = db[publicMethod]({ limit })

        // Use next() because all() auto-closes and thus can't be used twice atm
        for (let i = 0; i < limit; i++) await it.next()

        t.same(await it.all(), [])
      })
    }

    test(`${mode}().nextv() reduces size for _nextv() when near limit (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _nextv (size, options, callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, Array(size).fill(['a', 'a']))
          } else {
            this.nextTick(callback, null, Array(size).fill('a'))
          }
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      const it = db[publicMethod]({ limit: 3 })

      t.is((await it.nextv(2)).length, 2)
      t.is((await it.nextv(2)).length, 1)
      t.is((await it.nextv(2)).length, 0)
    })

    test(`${mode}().count increments by next(), nextv() and all() (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, 'a', 'a')
          } else {
            this.nextTick(callback, null, 'a')
          }
        }

        _nextv (size, options, callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, [['a', 'a'], ['b', 'b']])
          } else {
            this.nextTick(callback, null, ['a', 'b'])
          }
        }

        _all (options, callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, [['c', 'c'], ['d', 'd'], ['e', 'e']])
          } else {
            this.nextTick(callback, null, ['c', 'd', 'e'])
          }
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      const it = db[publicMethod]()

      for (let i = 0; i < 2; i++) {
        t.isNot(await it.next(), undefined) // 2 * 1 = 2
        t.is((await it.nextv(2)).length, 2) // 2 * 2 = 4
      }

      t.is(it.count, 2 + 4)
      t.is((await it.all()).length, 3)
      t.is(it.count, 2 + 4 + 3)
    })

    test(`${mode}() forwards encoding options (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(3)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.keyEncoding, 'utf8')
          t.is(options.valueEncoding, 'buffer')

          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, '281', Buffer.from('a'))
          } else if (mode === 'keys') {
            this.nextTick(callback, null, '281')
          } else {
            this.nextTick(callback, null, Buffer.from('a'))
          }
        }
      }

      const db = new MockLevel(dualManifest)
      if (!deferred) await db.open()

      const item = await db[publicMethod]({ keyEncoding: 'json', valueEncoding: 'hex' }).next()
      t.same(item, mode === 'iterator' ? [281, '61'] : mode === 'keys' ? 281 : '61')
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() with custom encodings that want a buffer (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(5)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.keyEncoding, 'buffer')
          t.is(options.valueEncoding, 'buffer')

          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, Buffer.from('a'), Buffer.from('b'))
          } else if (mode === 'keys') {
            this.nextTick(callback, null, Buffer.from('a'))
          } else {
            this.nextTick(callback, null, Buffer.from('b'))
          }
        }
      }

      const db = new MockLevel(dualManifest)
      const encoding = { encode: spy(identity), decode: spy(identity), format: 'buffer' }
      if (!deferred) await db.open()

      const it = db[publicMethod]({ keyEncoding: encoding, valueEncoding: encoding })
      const item = await it.next()

      t.is(encoding.encode.calls, 0, 'did not need to encode anything')
      t.is(encoding.decode.calls, mode === 'iterator' ? 2 : 1)
      t.same(item, mode === 'iterator' ? [Buffer.from('a'), Buffer.from('b')] : Buffer.from(mode === 'keys' ? 'a' : 'b'))
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() with custom encodings that want a string (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(5)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.keyEncoding, 'utf8')
          t.is(options.valueEncoding, 'utf8')

          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, 'a', 'b')
          } else if (mode === 'keys') {
            this.nextTick(callback, null, 'a')
          } else {
            this.nextTick(callback, null, 'b')
          }
        }
      }

      const db = new MockLevel(dualManifest)
      const encoding = { encode: spy(identity), decode: spy(identity), format: 'utf8' }
      if (!deferred) await db.open()

      const it = db[publicMethod]({ keyEncoding: encoding, valueEncoding: encoding })
      const item = await it.next()

      t.is(encoding.encode.calls, 0, 'did not need to encode anything')
      t.is(encoding.decode.calls, mode === 'iterator' ? 2 : 1)
      t.same(item, mode === 'iterator' ? ['a', 'b'] : mode === 'keys' ? 'a' : 'b')
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() encodes range options (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(6)

      let calls = 0
      const keyEncoding = {
        format: 'utf8',
        encode (key) {
          calls++
          return 'encoded_' + key
        },
        decode: identity
      }

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.gt, 'encoded_3')
          t.is(options.gte, 'encoded_4')
          t.is(options.lt, 'encoded_5')
          t.is(options.lte, 'encoded_6')
          t.is(options.foo, 7)
          return new Ctor(this, options)
        }
      }

      const db = new MockLevel(utf8Manifest, { keyEncoding })
      if (!deferred) await db.open()
      await db[publicMethod]({ gt: 3, gte: 4, lt: 5, lte: 6, foo: 7 }).next()
      t.is(calls, 4)
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() does not strip nullish range options (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(12)

      const db1 = mockLevel({
        [privateMethod] (options) {
          t.is(options.gt, '\x00', 'encoded null')
          t.is(options.gte, '\x00', 'encoded null')
          t.is(options.lt, '\x00', 'encoded null')
          t.is(options.lte, '\x00', 'encoded null')

          return new Ctor(this, options)
        }
      }, utf8Manifest, { keyEncoding: nullishEncoding, valueEncoding: nullishEncoding })

      const db2 = mockLevel({
        [privateMethod] (options) {
          t.is(hasOwnProperty.call(options, 'gt'), true)
          t.is(hasOwnProperty.call(options, 'gte'), true)
          t.is(hasOwnProperty.call(options, 'lt'), true)
          t.is(hasOwnProperty.call(options, 'lte'), true)

          t.is(options.gt, '\xff', 'encoded undefined')
          t.is(options.gte, '\xff', 'encoded undefined')
          t.is(options.lt, '\xff', 'encoded undefined')
          t.is(options.lte, '\xff', 'encoded undefined')

          return new Ctor(this, options)
        }
      }, utf8Manifest, { keyEncoding: nullishEncoding, valueEncoding: nullishEncoding })

      if (!deferred) {
        await Promise.all([db1.open(), db2.open()])
      }

      const promise1 = db1[publicMethod]({
        gt: null,
        gte: null,
        lt: null,
        lte: null
      }).next()

      const promise2 = db2[publicMethod]({
        gt: undefined,
        gte: undefined,
        lt: undefined,
        lte: undefined
      }).next()

      return Promise.all([promise1, promise2])
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() does not add nullish range options (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(4)

      const db = mockLevel({
        [privateMethod] (options) {
          t.is(hasOwnProperty.call(options, 'gt'), false)
          t.is(hasOwnProperty.call(options, 'gte'), false)
          t.is(hasOwnProperty.call(options, 'lt'), false)
          t.is(hasOwnProperty.call(options, 'lte'), false)

          return new Ctor(this, options)
        }
      })

      if (!deferred) await db.open()
      await db[publicMethod]({}).next()
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() encodes seek target (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(2)

      const db = mockLevel({
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }, utf8Manifest, { keyEncoding: 'json' })

      class MockIterator extends Ctor {
        _seek (target, options) {
          t.is(target, '"a"', 'encoded once')
          t.same(options, { keyEncoding: 'utf8' })
        }
      }

      if (!deferred) await db.open()
      const it = db[publicMethod]()
      it.seek('a')
      await it.next()
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() encodes seek target with custom encoding (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(1)

      const targets = []
      const db = mockLevel({
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }, utf8Manifest)

      class MockIterator extends Ctor {
        _seek (target) {
          targets.push(target)
        }
      }

      if (!deferred) await db.open()

      db[publicMethod]().seek('a')
      db[publicMethod]({ keyEncoding: 'json' }).seek('a')
      db[publicMethod]().seek('b', { keyEncoding: 'json' })

      await db.open()
      t.same(targets, ['a', '"a"', '"b"'], 'encoded targets')
    })

    // NOTE: adapted from encoding-down
    test(`${mode}() encodes nullish seek target (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(1)

      const targets = []
      const db = mockLevel({
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }, utf8Manifest, { keyEncoding: { encode: String, decode: identity, format: 'utf8' } })

      class MockIterator extends Ctor {
        _seek (target) {
          targets.push(target)
        }
      }

      if (!deferred) await db.open()

      // Unlike keys, nullish targets should not be rejected;
      // assume that the encoding gives these types meaning.
      db[publicMethod]().seek(null)
      db[publicMethod]().seek(undefined)

      await db.open()
      t.same(targets, ['null', 'undefined'], 'encoded')
    })

    test(`${mode}() has default nextv() (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      const sizes = [[1, [0]], [1, [1]], [2, [2]], [3, [3]]]
      t.plan(sizes.length * 2)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      let pos = 0
      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, 'k' + pos, 'v' + (pos++))
          } else if (mode === 'keys') {
            this.nextTick(callback, null, 'k' + (pos++))
          } else {
            this.nextTick(callback, null, 'v' + (pos++))
          }
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      let expectedPos = 0
      const it = db[publicMethod]()

      for (const [size, args] of sizes) {
        const actual = await it.nextv(...args)
        const expected = []

        for (let i = 0; i < size; i++) {
          const pos = expectedPos++
          if (mode === 'iterator') expected.push(['k' + pos, 'v' + pos])
          else if (mode === 'keys') expected.push('k' + pos)
          else expected.push('v' + pos)
        }

        t.is(actual.length, size)
        t.same(actual, expected)
      }
    })

    test(`${mode}() default nextv() forwards next() error (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(2)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          t.pass('called')
          this.nextTick(callback, new Error('test'))
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      try {
        await db[publicMethod]().nextv(10)
      } catch (err) {
        t.is(err.message, 'test')
      }
    })

    test(`${mode}() has default all() (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(8)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      let pos = 0
      class MockIterator extends Ctor {
        _nextv (size, options, callback) {
          t.is(size, 1000)
          t.same(options, {})

          if (pos === 4) {
            this.nextTick(callback, null, [])
          } else if (mode === 'iterator' || def) {
            this.nextTick(callback, null, [[String(pos++), 'a'], [String(pos++), 'b']])
          } else if (mode === 'keys') {
            this.nextTick(callback, null, [String(pos++), String(pos++)])
          } else {
            pos += 2
            this.nextTick(callback, null, ['a', 'b'])
          }
        }

        _close (callback) {
          t.pass('closed')
          this.nextTick(callback)
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      t.same(await db[publicMethod]().all(), [
        ['0', 'a'],
        ['1', 'b'],
        ['2', 'a'],
        ['3', 'b']
      ].map(kv => mode === 'iterator' ? kv : kv[mode === 'keys' ? 0 : 1]))
    })

    test(`${mode}() default all() forwards nextv() error (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(2)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _nextv (size, options, callback) {
          t.pass('called')
          this.nextTick(callback, new Error('test'))
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      try {
        await db[publicMethod]().all()
      } catch (err) {
        t.is(err.message, 'test')
      }
    })

    test(`${mode}() custom all() (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(3)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _all (options, callback) {
          t.same(options, {})

          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, [['k0', 'v0'], ['k1', 'v1']])
          } else if (mode === 'keys') {
            this.nextTick(callback, null, ['k0', 'k1'])
          } else {
            this.nextTick(callback, null, ['v0', 'v1'])
          }
        }

        _close (callback) {
          t.pass('closed')
          this.nextTick(callback)
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      t.same(await db[publicMethod]().all(), [
        ['k0', 'v0'],
        ['k1', 'v1']
      ].map(kv => mode === 'iterator' ? kv : kv[mode === 'keys' ? 0 : 1]))
    })

    test(`${mode}() custom all() forwards error and closes (deferred: ${deferred}, default implementation: ${def})`, async function (t) {
      t.plan(3)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _all (options, callback) {
          t.pass('_all called')
          this.nextTick(callback, new Error('test'))
        }

        _close (callback) {
          t.pass('closed')
          this.nextTick(callback)
        }
      }

      const db = new MockLevel(utf8Manifest)
      if (!deferred) await db.open()

      try {
        await db[publicMethod]().all()
      } catch (err) {
        t.is(err.message, 'test')
      }
    })
  }
}

for (const deferred of [false, true]) {
  // NOTE: adapted from encoding-down
  test(`iterator() skips decoding keys if options.keys is false (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const keyEncoding = {
      format: 'utf8',
      decode (key) {
        t.fail('should not be called')
      },
      encode: identity
    }

    const db = mockLevel({
      _iterator (options) {
        t.is(options.keys, false)

        return mockIterator(this, options, {
          _next (callback) {
            this.nextTick(callback, null, '', 'value')
          }
        })
      }
    }, utf8Manifest, { keyEncoding })

    if (!deferred) await db.open()
    const [key, value] = await db.iterator({ keys: false }).next()

    t.is(key, undefined, 'normalized key to undefined')
    t.is(value, 'value', 'got value')
  })

  // NOTE: adapted from encoding-down
  test(`iterator() skips decoding values if options.values is false (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const valueEncoding = {
      format: 'utf8',
      decode (value) {
        t.fail('should not be called')
      },
      encode: identity
    }

    const db = mockLevel({
      _iterator (options) {
        t.is(options.values, false)

        return mockIterator(this, options, {
          _next (callback) {
            callback(null, 'key', '')
          }
        })
      }
    }, utf8Manifest, { valueEncoding })

    if (!deferred) await db.open()
    const [key, value] = await db.iterator({ values: false }).next()

    t.is(key, 'key', 'got key')
    t.is(value, undefined, 'normalized value to undefined')
  })
}

function spy (fn) {
  const wrapped = function (...args) {
    wrapped.calls++
    return fn(...args)
  }
  wrapped.calls = 0
  return wrapped
}
