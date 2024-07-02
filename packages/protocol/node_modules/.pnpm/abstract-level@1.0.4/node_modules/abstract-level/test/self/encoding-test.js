'use strict'

// TODO: move to per-method test files

const test = require('tape')
const { Buffer } = require('buffer')
const { mockLevel, mockChainedBatch, nullishEncoding } = require('../util')
const identity = (v) => v

const utf8Manifest = { encodings: { utf8: true } }
const dualManifest = { encodings: { utf8: true, buffer: true } }
const hasOwnProperty = Object.prototype.hasOwnProperty

for (const deferred of [false, true]) {
  // NOTE: adapted from encoding-down
  test(`get() encodes utf8 key (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _get (key, options, callback) {
        t.is(key, '8')
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback, null, 'foo')
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    t.same(await db.get(8), 'foo')
  })

  // NOTE: adapted from encoding-down
  test(`get() takes encoding options (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _get (key, options, callback) {
        t.is(key, '[1,"2"]')
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback, null, '123')
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    t.same(await db.get([1, '2'], { keyEncoding: 'json', valueEncoding: 'json' }), 123)
  })

  // NOTE: adapted from encoding-down
  test(`get() with custom value encoding that wants a buffer (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const db = mockLevel({
      _get (key, options, callback) {
        t.same(key, 'key')
        t.same(options, { keyEncoding: 'utf8', valueEncoding: 'buffer' })
        this.nextTick(callback, null, Buffer.alloc(1))
      }
    }, dualManifest, {
      keyEncoding: 'utf8',
      valueEncoding: { encode: identity, decode: identity, format: 'buffer' }
    })

    if (!deferred) await db.open()
    t.same(await db.get('key'), Buffer.alloc(1))
  })

  // NOTE: adapted from encoding-down
  test(`get() with custom value encoding that wants a string (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const db = mockLevel({
      _get (key, options, callback) {
        t.same(key, Buffer.from('key'))
        t.same(options, { keyEncoding: 'buffer', valueEncoding: 'utf8' })
        this.nextTick(callback, null, 'x')
      }
    }, dualManifest, {
      keyEncoding: 'buffer',
      valueEncoding: { encode: identity, decode: identity, format: 'utf8' }
    })

    if (!deferred) await db.open()
    t.same(await db.get(Buffer.from('key')), 'x')
  })

  // NOTE: adapted from encoding-down
  test(`put() encodes utf8 key and value (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _put (key, value, options, callback) {
        t.is(key, '8')
        t.is(value, '4')
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.put(8, 4)
  })

  // NOTE: adapted from encoding-down
  test(`put() takes encoding options (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _put (key, value, options, callback) {
        t.is(key, '[1,"2"]')
        t.is(value, '{"x":3}')
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.put([1, '2'], { x: 3 }, { keyEncoding: 'json', valueEncoding: 'json' })
  })

  // NOTE: adapted from encoding-down
  test(`del() encodes utf8 key (deferred: ${deferred})`, async function (t) {
    t.plan(2)

    const db = mockLevel({
      _del (key, options, callback) {
        t.is(key, '2')
        t.is(options.keyEncoding, 'utf8')
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.del(2)
  })

  // NOTE: adapted from encoding-down
  test(`del() takes keyEncoding option (deferred: ${deferred})`, async function (t) {
    t.plan(2)

    const db = mockLevel({
      _del (key, options, callback) {
        t.is(key, '[1,"2"]')
        t.is(options.keyEncoding, 'utf8')
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.del([1, '2'], { keyEncoding: 'json' })
  })

  test(`getMany() encodes utf8 key (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _getMany (keys, options, callback) {
        t.same(keys, ['8', '29'])
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback, null, ['foo', 'bar'])
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    t.same(await db.getMany([8, 29]), ['foo', 'bar'])
  })

  test(`getMany() takes encoding options (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _getMany (keys, options, callback) {
        t.same(keys, ['[1,"2"]', '"x"'])
        t.is(options.keyEncoding, 'utf8')
        t.is(options.valueEncoding, 'utf8')
        this.nextTick(callback, null, ['123', '"hi"'])
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    t.same(await db.getMany([[1, '2'], 'x'], { keyEncoding: 'json', valueEncoding: 'json' }), [123, 'hi'])
  })

  test(`getMany() with custom value encoding that wants a buffer (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const db = mockLevel({
      _getMany (keys, options, callback) {
        t.same(keys, ['key'])
        t.same(options, { keyEncoding: 'utf8', valueEncoding: 'buffer' })
        this.nextTick(callback, null, [Buffer.alloc(1)])
      }
    }, dualManifest, {
      keyEncoding: 'utf8',
      valueEncoding: { encode: identity, decode: identity, format: 'buffer' }
    })

    if (!deferred) await db.open()
    t.same(await db.getMany(['key']), [Buffer.alloc(1)])
  })

  test(`getMany() with custom value encoding that wants a string (deferred: ${deferred})`, async function (t) {
    t.plan(3)

    const db = mockLevel({
      _getMany (keys, options, callback) {
        t.same(keys, [Buffer.from('key')])
        t.same(options, { keyEncoding: 'buffer', valueEncoding: 'utf8' })
        this.nextTick(callback, null, ['x'])
      }
    }, dualManifest, {
      keyEncoding: 'buffer',
      valueEncoding: { encode: identity, decode: identity, format: 'utf8' }
    })

    if (!deferred) await db.open()
    t.same(await db.getMany([Buffer.from('key')]), ['x'])
  })

  // NOTE: adapted from encoding-down
  test(`chainedBatch.put() and del() encode utf8 key and value (deferred: ${deferred})`, async function (t) {
    t.plan(deferred ? 2 : 4)

    let db

    if (deferred) {
      db = mockLevel({
        _batch (array, options, callback) {
          t.same(array, [
            { type: 'put', key: '1', value: '2', keyEncoding: 'utf8', valueEncoding: 'utf8' },
            { type: 'del', key: '3', keyEncoding: 'utf8' }
          ])
          t.same(options, {})
          this.nextTick(callback)
        }
      }, utf8Manifest)
    } else {
      db = mockLevel({
        _chainedBatch () {
          return mockChainedBatch(this, {
            _put: function (key, value, options) {
              t.same({ key, value }, { key: '1', value: '2' })
              t.same(options, { keyEncoding: 'utf8', valueEncoding: 'utf8' })
            },
            _del: function (key, options) {
              t.is(key, '3')
              t.same(options, { keyEncoding: 'utf8' })
            }
          })
        }
      }, utf8Manifest)
    }

    if (!deferred) await db.open()
    await db.batch().put(1, 2).del(3).write()
  })

  // NOTE: adapted from encoding-down
  test(`chainedBatch.put() and del() take encoding options (deferred: ${deferred})`, async function (t) {
    t.plan(deferred ? 2 : 4)

    let db

    const putOptions = { keyEncoding: 'json', valueEncoding: 'json' }
    const delOptions = { keyEncoding: 'json' }

    if (deferred) {
      db = mockLevel({
        _batch (array, options, callback) {
          t.same(array, [
            { type: 'put', key: '"1"', value: '{"x":[2]}', keyEncoding: 'utf8', valueEncoding: 'utf8' },
            { type: 'del', key: '"3"', keyEncoding: 'utf8' }
          ])
          t.same(options, {})
          this.nextTick(callback)
        }
      }, utf8Manifest)
    } else {
      db = mockLevel({
        _chainedBatch () {
          return mockChainedBatch(this, {
            _put: function (key, value, options) {
              t.same({ key, value }, { key: '"1"', value: '{"x":[2]}' })
              t.same(options, { keyEncoding: 'utf8', valueEncoding: 'utf8' })
            },
            _del: function (key, options) {
              t.is(key, '"3"')
              t.same(options, { keyEncoding: 'utf8' })
            }
          })
        }
      }, utf8Manifest)
    }

    if (!deferred) await db.open()
    await db.batch().put('1', { x: [2] }, putOptions).del('3', delOptions).write()
  })

  // NOTE: adapted from encoding-down
  test(`clear() receives keyEncoding option (deferred: ${deferred})`, async function (t) {
    t.plan(1)

    const db = mockLevel({
      _clear: function (options, callback) {
        t.same(options, { keyEncoding: 'utf8', reverse: false, limit: -1 })
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.clear()
  })

  test(`clear() takes keyEncoding option (deferred: ${deferred})`, async function (t) {
    t.plan(1)

    const db = mockLevel({
      _clear: function (options, callback) {
        t.same(options, { keyEncoding: 'utf8', gt: '"a"', reverse: false, limit: -1 })
        this.nextTick(callback)
      }
    }, utf8Manifest)

    if (!deferred) await db.open()
    await db.clear({ keyEncoding: 'json', gt: 'a' })
  })

  // NOTE: adapted from encoding-down
  test(`clear() encodes range options (deferred: ${deferred})`, async function (t) {
    t.plan(5)

    const keyEncoding = {
      format: 'utf8',
      encode: function (key) {
        return 'encoded_' + key
      },
      decode: identity
    }

    const db = mockLevel({
      _clear: function (options, callback) {
        t.is(options.gt, 'encoded_1')
        t.is(options.gte, 'encoded_2')
        t.is(options.lt, 'encoded_3')
        t.is(options.lte, 'encoded_4')
        t.is(options.foo, 5)
        this.nextTick(callback)
      }
    }, utf8Manifest, { keyEncoding })

    if (!deferred) await db.open()
    await db.clear({ gt: 1, gte: 2, lt: 3, lte: 4, foo: 5 })
  })

  // NOTE: adapted from encoding-down
  test(`clear() does not strip nullish range options (deferred: ${deferred})`, async function (t) {
    t.plan(12)

    const db1 = mockLevel({
      _clear: function (options, callback) {
        t.is(options.gt, '\x00', 'encoded null')
        t.is(options.gte, '\x00', 'encoded null')
        t.is(options.lt, '\x00', 'encoded null')
        t.is(options.lte, '\x00', 'encoded null')
        this.nextTick(callback)
      }
    }, utf8Manifest, { keyEncoding: nullishEncoding, valueEncoding: nullishEncoding })

    const db2 = mockLevel({
      _clear: function (options, callback) {
        t.is(hasOwnProperty.call(options, 'gt'), true)
        t.is(hasOwnProperty.call(options, 'gte'), true)
        t.is(hasOwnProperty.call(options, 'lt'), true)
        t.is(hasOwnProperty.call(options, 'lte'), true)

        t.is(options.gt, '\xff', 'encoded undefined')
        t.is(options.gte, '\xff', 'encoded undefined')
        t.is(options.lt, '\xff', 'encoded undefined')
        t.is(options.lte, '\xff', 'encoded undefined')

        this.nextTick(callback)
      }
    }, utf8Manifest, { keyEncoding: nullishEncoding, valueEncoding: nullishEncoding })

    if (!deferred) {
      await Promise.all([db1.open(), db2.open()])
    }

    const promise1 = db1.clear({
      gt: null,
      gte: null,
      lt: null,
      lte: null
    })

    const promise2 = db2.clear({
      gt: undefined,
      gte: undefined,
      lt: undefined,
      lte: undefined
    })

    await Promise.all([promise1, promise2])
  })

  // NOTE: adapted from encoding-down
  test(`clear() does not add nullish range options (deferred: ${deferred})`, async function (t) {
    t.plan(4)

    const db = mockLevel({
      _clear: function (options, callback) {
        t.is(hasOwnProperty.call(options, 'gt'), false)
        t.is(hasOwnProperty.call(options, 'gte'), false)
        t.is(hasOwnProperty.call(options, 'lt'), false)
        t.is(hasOwnProperty.call(options, 'lte'), false)
        this.nextTick(callback)
      }
    })

    if (!deferred) await db.open()
    await db.clear({})
  })
}
