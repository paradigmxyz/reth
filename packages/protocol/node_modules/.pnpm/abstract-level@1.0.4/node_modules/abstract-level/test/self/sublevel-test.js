'use strict'

const test = require('tape')
const { Buffer } = require('buffer')
const { AbstractLevel, AbstractSublevel } = require('../..')
const { AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('../..')
const nextTick = AbstractLevel.prototype.nextTick

class NoopLevel extends AbstractLevel {
  constructor (...args) {
    super(
      { encodings: { utf8: true, buffer: true, view: true } },
      ...args
    )
  }
}

test('sublevel is extensible', function (t) {
  t.plan(6)

  class MockLevel extends AbstractLevel {
    _sublevel (name, options) {
      t.is(name, 'test')
      t.same(options, { separator: '!', customOption: 123 })

      return new MockSublevel(this, name, {
        ...options,
        manifest: {
          encodings: { ignored: true },
          additionalMethods: { test: true },
          events: { foo: true }
        }
      })
    }
  }

  class MockSublevel extends AbstractSublevel {
    test () {
      this.emit('foo')
    }
  }

  const db = new MockLevel({
    encodings: { utf8: true },
    additionalMethods: { ignored: true },
    events: { ignored: true }
  })

  const sub = db.sublevel('test', { customOption: 123 })

  t.is(sub.supports.encodings.ignored, undefined)
  t.same(sub.supports.additionalMethods, { test: true })
  t.same(sub.supports.events, {
    foo: true,

    // Added by AbstractLevel
    opening: true,
    open: true,
    closing: true,
    closed: true,
    put: true,
    del: true,
    batch: true,
    clear: true
  })

  sub.on('foo', () => t.pass('emitted'))
  sub.test()
})

// NOTE: adapted from subleveldown
test('sublevel prefix and options', function (t) {
  t.test('empty prefix', function (t) {
    const sub = new NoopLevel().sublevel('')
    t.is(sub.prefix, '!!')
    t.end()
  })

  t.test('prefix without options', function (t) {
    const sub = new NoopLevel().sublevel('prefix')
    t.is(sub.prefix, '!prefix!')
    t.end()
  })

  t.test('prefix and separator option', function (t) {
    const sub = new NoopLevel().sublevel('prefix', { separator: '%' })
    t.is(sub.prefix, '%prefix%')
    t.end()
  })

  t.test('separator is trimmed from prefix', function (t) {
    const sub1 = new NoopLevel().sublevel('!prefix')
    t.is(sub1.prefix, '!prefix!')

    const sub2 = new NoopLevel().sublevel('prefix!')
    t.is(sub2.prefix, '!prefix!')

    const sub3 = new NoopLevel().sublevel('!!prefix!!')
    t.is(sub3.prefix, '!prefix!')

    const sub4 = new NoopLevel().sublevel('@prefix@', { separator: '@' })
    t.is(sub4.prefix, '@prefix@')

    t.end()
  })

  t.test('repeated separator can not result in empty prefix', function (t) {
    const sub = new NoopLevel().sublevel('!!!!')
    t.is(sub.prefix, '!!')
    t.end()
  })

  t.test('invalid sublevel prefix', function (t) {
    t.throws(() => new NoopLevel().sublevel('foo\x05'), (err) => err.code === 'LEVEL_INVALID_PREFIX')
    t.throws(() => new NoopLevel().sublevel('foo\xff'), (err) => err.code === 'LEVEL_INVALID_PREFIX')
    t.throws(() => new NoopLevel().sublevel('foo!', { separator: '@' }), (err) => err.code === 'LEVEL_INVALID_PREFIX')
    t.end()
  })

  t.test('legacy sublevel(down) options', function (t) {
    t.throws(() => new NoopLevel().sublevel('foo', 'bar'), (err) => err.code === 'LEVEL_LEGACY')
    t.throws(() => new NoopLevel().sublevel('foo', { open: () => {} }), (err) => err.code === 'LEVEL_LEGACY')
    t.end()
  })

  // See https://github.com/Level/subleveldown/issues/78
  t.test('doubly nested sublevel has correct prefix', async function (t) {
    t.plan(1)

    const keys = []
    class MockLevel extends AbstractLevel {
      _put (key, value, options, callback) {
        keys.push(key)
        nextTick(callback)
      }
    }

    const db = new MockLevel({ encodings: { utf8: true } })
    const sub1 = db.sublevel('1')
    const sub2 = sub1.sublevel('2')
    const sub3 = sub2.sublevel('3')

    await sub1.put('a', 'value')
    await sub2.put('b', 'value')
    await sub3.put('c', 'value')

    t.same(keys.sort(), [
      '!1!!2!!3!c',
      '!1!!2!b',
      '!1!a'
    ])
  })

  t.end()
})

test('sublevel.prefixKey()', function (t) {
  const db = new AbstractLevel({ encodings: { utf8: true, buffer: true, view: true } })
  const sub = db.sublevel('test')
  const textEncoder = new TextEncoder()

  t.same(sub.prefixKey('', 'utf8'), '!test!')
  t.same(sub.prefixKey('a', 'utf8'), '!test!a')

  t.same(sub.prefixKey(Buffer.from(''), 'buffer'), Buffer.from('!test!'))
  t.same(sub.prefixKey(Buffer.from('a'), 'buffer'), Buffer.from('!test!a'))

  t.same(sub.prefixKey(textEncoder.encode(''), 'view'), textEncoder.encode('!test!'))
  t.same(sub.prefixKey(textEncoder.encode('a'), 'view'), textEncoder.encode('!test!a'))

  t.end()
})

// NOTE: adapted from subleveldown
test('sublevel manifest and parent db', function (t) {
  t.test('sublevel inherits manifest from parent db', function (t) {
    const parent = new AbstractLevel({
      encodings: { utf8: true },
      seek: true,
      foo: true
    })
    const sub = parent.sublevel('')
    t.is(sub.supports.foo, true, 'AbstractSublevel inherits from parent')
    t.is(sub.supports.seek, true, 'AbstractSublevel inherits from parent')
    t.end()
  })

  t.test('sublevel does not support additionalMethods', function (t) {
    const parent = new AbstractLevel({
      encodings: { utf8: true },
      additionalMethods: { foo: true }
    })

    // We're expecting that AbstractSublevel removes the additionalMethod
    // because it can't automatically prefix any key(-like) arguments
    const sub = parent.sublevel('')
    t.same(sub.supports.additionalMethods, {})
    t.same(parent.supports.additionalMethods, { foo: true })
    t.is(typeof sub.foo, 'undefined', 'AbstractSublevel does not expose method')
    t.end()
  })

  t.test('sublevel.db is set to parent db', function (t) {
    const db = new NoopLevel()
    const sub = db.sublevel('test')
    sub.once('open', function () {
      t.ok(sub.db instanceof NoopLevel)
      t.end()
    })
  })

  t.end()
})

// NOTE: adapted from subleveldown
test('opening & closing sublevel', function (t) {
  t.test('error from open() does not bubble up to sublevel', function (t) {
    t.plan(5)

    class MockLevel extends AbstractLevel {
      _open (opts, cb) {
        nextTick(cb, new Error('error from underlying store'))
      }
    }

    const db = new MockLevel({ encodings: { buffer: true } })
    const sub = db.sublevel('test')

    db.open((err) => {
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
      t.is(err && err.cause && err.cause.message, 'error from underlying store')
    })

    sub.open((err) => {
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
      t.is(err && err.cause && err.cause.code, 'LEVEL_DATABASE_NOT_OPEN') // from db
      t.is(err && err.cause && err.cause.cause, undefined) // but does not have underlying error
    })
  })

  t.test('cannot create a sublevel on a closed db', function (t) {
    t.plan(4)

    const db = new NoopLevel()

    db.once('open', function () {
      db.close(function (err) {
        t.error(err, 'no error')

        db.sublevel('test').open(function (err) {
          t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN', 'sublevel not opened')

          db.open(function (err) {
            t.error(err, 'no error')

            db.sublevel('test').on('open', function () {
              t.pass('sublevel opened')
            })
          })
        })
      })
    })
  })

  t.test('can close db and sublevel once opened', function (t) {
    t.plan(3)

    const db = new NoopLevel()

    db.open(function (err) {
      t.ifError(err, 'no open error')
      const sub = db.sublevel('test')

      sub.once('open', function () {
        db.close(function (err) {
          t.ifError(err, 'no close error')
        })

        sub.close(function (err) {
          t.ifError(err, 'no close error')
        })
      })
    })
  })

  t.test('sublevel rejects operations if parent db is closed', function (t) {
    t.plan(9)

    const db = new NoopLevel()

    db.open(function (err) {
      t.ifError(err, 'no open error')

      const sub = db.sublevel('test')
      const it = sub.iterator()

      sub.once('open', function () {
        db.close(function (err) {
          t.ifError(err, 'no close error')

          sub.put('foo', 'bar', verify)
          sub.get('foo', verify)
          sub.del('foo', verify)
          sub.clear(verify)
          sub.batch([{ type: 'del', key: 'foo' }], verify)

          it.next(function (err) {
            t.is(err.code, 'LEVEL_ITERATOR_NOT_OPEN')
            it.close(t.ifError.bind(t))
          })

          function verify (err) {
            t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
          }
        })
      })
    })
  })

  t.test('cannot close db while sublevel is opening', function (t) {
    t.plan(5)

    const db = new NoopLevel()

    db.open(function (err) {
      t.ifError(err, 'no open error')
      const sub = db.sublevel('test')

      sub.open((err) => {
        t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
      })

      db.close(function (err) {
        t.ifError(err, 'no close error')
        t.is(sub.status, 'closed')
        t.is(db.status, 'closed')
      })
    })
  })

  t.test('cannot create sublevel while db is closing', function (t) {
    t.plan(5)

    const db = new NoopLevel()

    db.open(function (err) {
      t.ifError(err, 'no open error')

      db.close(function (err) {
        t.ifError(err, 'no close error')
        t.is(db.status, 'closed')
      })

      const sub = db.sublevel('test')

      sub.open((err) => {
        t.is(err.code, 'LEVEL_DATABASE_NOT_OPEN')
        t.is(sub.status, 'closed')
      })
    })
  })

  t.test('can wrap a sublevel and reopen the wrapped sublevel', function (t) {
    const db = new NoopLevel()
    const sub1 = db.sublevel('test1')
    const sub2 = sub1.sublevel('test2')

    sub2.once('open', function () {
      verify()

      sub2.close(function (err) {
        t.ifError(err, 'no close error')

        // Prefixes should be the same after closing & reopening
        // See https://github.com/Level/subleveldown/issues/78
        sub2.open(function (err) {
          t.ifError(err, 'no open error')
          verify()
          t.end()
        })
      })
    })

    function verify () {
      t.is(sub1.prefix, '!test1!', 'sub1 prefix ok')
      t.ok(sub1.db instanceof NoopLevel)
      t.is(sub2.prefix, '!test1!!test2!', 'sub2 prefix ok')
      t.ok(sub2.db instanceof NoopLevel)
    }
  })

  // Also test default fallback implementations of keys() and values()
  for (const [mode, def] of [['iterator', false], ['keys', false], ['values', false], ['keys', true], ['values', true]]) {
    const Ctor = mode === 'iterator' || def ? AbstractIterator : mode === 'keys' ? AbstractKeyIterator : AbstractValueIterator
    const privateMethod = def ? '_iterator' : '_' + mode
    const publicMethod = mode

    t.test(`error from sublevel.${mode}() bubbles up (default implementation: ${def})`, function (t) {
      t.plan(2)

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      }

      class MockIterator extends Ctor {
        _next (callback) {
          this.nextTick(callback, new Error('next() error from parent database'))
        }
      }

      const db = new MockLevel({ encodings: { buffer: true } })
      const sub = db.sublevel('test')
      const it = sub[publicMethod]()

      it.next(function (err) {
        t.is(err.message, 'next() error from parent database')

        it.close(function () {
          t.pass('closed')
        })
      })
    })
  }

  t.end()
})

test('sublevel operations are prefixed', function (t) {
  t.test('sublevel.getMany() is prefixed', async function (t) {
    t.plan(2)

    class MockLevel extends AbstractLevel {
      _getMany (keys, options, callback) {
        t.same(keys, ['!test!a', '!test!b'])
        t.same(options, { keyEncoding: 'utf8', valueEncoding: 'utf8' })
        nextTick(callback, null, ['1', '2'])
      }
    }

    const db = new MockLevel({ encodings: { utf8: true } })
    const sub = db.sublevel('test')

    await sub.open()
    await sub.getMany(['a', 'b'])
  })

  // Also test default fallback implementations of keys() and values()
  for (const [mode, def] of [['iterator', false], ['keys', false], ['values', false], ['keys', true], ['values', true]]) {
    const Ctor = mode === 'iterator' || def ? AbstractIterator : mode === 'keys' ? AbstractKeyIterator : AbstractValueIterator
    const privateMethod = def ? '_iterator' : '_' + mode
    const publicMethod = mode

    for (const deferred of [false, true]) {
      t.test(`sublevel ${mode}.seek() target is prefixed (default implementation: ${def}, deferred: ${deferred})`, async function (t) {
        t.plan(2)

        class MockIterator extends Ctor {
          _seek (target, options) {
            t.is(target, '!sub!123')
            t.is(options.keyEncoding, 'utf8')
          }
        }

        class MockLevel extends AbstractLevel {
          [privateMethod] (options) {
            return new MockIterator(this, options)
          }
        }

        const db = new MockLevel({ encodings: { utf8: true } })
        const sub = db.sublevel('sub', { keyEncoding: 'json' })

        if (!deferred) await sub.open()

        const it = sub[publicMethod]()
        it.seek(123)

        if (deferred) await sub.open()
      })
    }
  }

  t.test('sublevel.clear() is prefixed', async function (t) {
    t.plan(4)

    const calls = []
    class MockLevel extends AbstractLevel {
      _clear (options, callback) {
        calls.push(options)
        nextTick(callback)
      }
    }

    const db = new MockLevel({ encodings: { utf8: true } })
    const sub = db.sublevel('sub')

    const test = async (options, expected) => {
      await sub.clear(options)
      t.same(calls.shift(), expected)
    }

    await sub.open()

    await test(undefined, {
      gte: '!sub!',
      lte: '!sub"',
      keyEncoding: 'utf8',
      reverse: false,
      limit: -1
    })

    await test({ gt: 'a' }, {
      gt: '!sub!a',
      lte: '!sub"',
      keyEncoding: 'utf8',
      reverse: false,
      limit: -1
    })

    await test({ gte: 'a', lt: 'x' }, {
      gte: '!sub!a',
      lt: '!sub!x',
      keyEncoding: 'utf8',
      reverse: false,
      limit: -1
    })

    await test({ lte: 'x' }, {
      gte: '!sub!',
      lte: '!sub!x',
      keyEncoding: 'utf8',
      reverse: false,
      limit: -1
    })
  })

  t.end()
})

test('sublevel encodings', function (t) {
  // NOTE: adapted from subleveldown
  t.test('different sublevels can have different encodings', function (t) {
    t.plan(10)

    const puts = []
    const gets = []

    class MockLevel extends AbstractLevel {
      _put (key, value, { keyEncoding, valueEncoding }, callback) {
        puts.push({ key, value, keyEncoding, valueEncoding })
        nextTick(callback)
      }

      _get (key, { keyEncoding, valueEncoding }, callback) {
        gets.push({ key, keyEncoding, valueEncoding })
        nextTick(callback, null, puts.shift().value)
      }
    }

    const db = new MockLevel({ encodings: { buffer: true, utf8: true } })
    const sub1 = db.sublevel('test1', { valueEncoding: 'json' })
    const sub2 = db.sublevel('test2', { keyEncoding: 'buffer', valueEncoding: 'buffer' })

    sub1.put('foo', { some: 'json' }, function (err) {
      t.error(err, 'no error')

      t.same(puts, [{
        key: '!test1!foo',
        value: '{"some":"json"}',
        keyEncoding: 'utf8',
        valueEncoding: 'utf8'
      }])

      sub1.get('foo', function (err, value) {
        t.error(err, 'no error')
        t.same(value, { some: 'json' })
        t.same(gets.shift(), {
          key: '!test1!foo',
          keyEncoding: 'utf8',
          valueEncoding: 'utf8'
        })

        sub2.put(Buffer.from([1, 2]), Buffer.from([3]), function (err) {
          t.error(err, 'no error')

          t.same(puts, [{
            key: Buffer.from('!test2!\x01\x02'),
            value: Buffer.from([3]),
            keyEncoding: 'buffer',
            valueEncoding: 'buffer'
          }])

          sub2.get(Buffer.from([1, 2]), function (err, value) {
            t.error(err, 'no error')
            t.same(value, Buffer.from([3]))
            t.same(gets.shift(), {
              key: Buffer.from('!test2!\x01\x02'),
              keyEncoding: 'buffer',
              valueEncoding: 'buffer'
            })
          })
        })
      })
    })
  })

  t.test('sublevel indirectly supports transcoded encoding', function (t) {
    t.plan(5)

    class MockLevel extends AbstractLevel {
      _put (key, value, { keyEncoding, valueEncoding }, callback) {
        t.same({ key, value, keyEncoding, valueEncoding }, {
          key: Buffer.from('!test!foo'),
          value: Buffer.from('{"some":"json"}'),
          keyEncoding: 'buffer',
          valueEncoding: 'buffer'
        })
        nextTick(callback)
      }

      _get (key, { keyEncoding, valueEncoding }, callback) {
        t.same({ key, keyEncoding, valueEncoding }, {
          key: Buffer.from('!test!foo'),
          keyEncoding: 'buffer',
          valueEncoding: 'buffer'
        })
        nextTick(callback, null, Buffer.from('{"some":"json"}'))
      }
    }

    const db = new MockLevel({ encodings: { buffer: true } })
    const sub = db.sublevel('test', { valueEncoding: 'json' })

    sub.put('foo', { some: 'json' }, function (err) {
      t.error(err, 'no error')

      sub.get('foo', function (err, value) {
        t.error(err, 'no error')
        t.same(value, { some: 'json' })
      })
    })
  })

  t.test('concatenating sublevel Buffer keys', function (t) {
    t.plan(10)

    const key = Buffer.from('00ff', 'hex')
    const prefixedKey = Buffer.concat([Buffer.from('!test!'), key])

    class MockLevel extends AbstractLevel {
      _put (key, value, options, callback) {
        t.is(options.keyEncoding, 'buffer')
        t.is(options.valueEncoding, 'buffer')
        t.same(key, prefixedKey)
        t.same(value, Buffer.from('bar'))
        nextTick(callback)
      }

      _get (key, options, callback) {
        t.is(options.keyEncoding, 'buffer')
        t.is(options.valueEncoding, 'buffer')
        t.same(key, prefixedKey)
        nextTick(callback, null, Buffer.from('bar'))
      }
    }

    const db = new MockLevel({ encodings: { buffer: true } })
    const sub = db.sublevel('test', { keyEncoding: 'buffer' })

    sub.put(key, 'bar', function (err) {
      t.ifError(err)
      sub.get(key, function (err, value) {
        t.ifError(err)
        t.is(value, 'bar')
      })
    })
  })

  t.test('concatenating sublevel Uint8Array keys', function (t) {
    t.plan(10)

    const key = new Uint8Array([0, 255])
    const textEncoder = new TextEncoder()
    const prefix = textEncoder.encode('!test!')
    const prefixedKey = new Uint8Array(prefix.byteLength + key.byteLength)

    prefixedKey.set(prefix, 0)
    prefixedKey.set(key, prefix.byteLength)

    class MockLevel extends AbstractLevel {
      _put (key, value, options, callback) {
        t.is(options.keyEncoding, 'view')
        t.is(options.valueEncoding, 'view')
        t.same(key, prefixedKey)
        t.same(value, textEncoder.encode('bar'))
        nextTick(callback)
      }

      _get (key, options, callback) {
        t.is(options.keyEncoding, 'view')
        t.is(options.valueEncoding, 'view')
        t.same(key, prefixedKey)
        nextTick(callback, null, textEncoder.encode('bar'))
      }
    }

    const db = new MockLevel({ encodings: { view: true } })
    const sub = db.sublevel('test', { keyEncoding: 'view' })

    sub.put(key, 'bar', function (err) {
      t.ifError(err)
      sub.get(key, function (err, value) {
        t.ifError(err)
        t.is(value, 'bar')
      })
    })
  })

  // Also test default fallback implementations of keys() and values()
  for (const [mode, def] of [['iterator', false], ['keys', false], ['values', false], ['keys', true], ['values', true]]) {
    const Ctor = mode === 'iterator' || def ? AbstractIterator : mode === 'keys' ? AbstractKeyIterator : AbstractValueIterator
    const privateMethod = def ? '_iterator' : '_' + mode
    const publicMethod = mode

    t.test(`unfixing sublevel.${mode}() Buffer keys (default implementation: ${def})`, function (t) {
      t.plan(4)

      const testKey = Buffer.from('00ff', 'hex')
      const prefixedKey = Buffer.concat([Buffer.from('!test!'), testKey])

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, prefixedKey, 'bar')
          } else if (mode === 'keys') {
            this.nextTick(callback, null, prefixedKey)
          } else {
            this.nextTick(callback, null, 'bar')
          }
        }
      }

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.keyEncoding, 'buffer')
          t.is(options.valueEncoding, 'utf8')
          return new MockIterator(this, options)
        }
      }

      const db = new MockLevel({ encodings: { buffer: true, view: true, utf8: true } })
      const sub = db.sublevel('test', { keyEncoding: 'buffer' })

      sub[publicMethod]().next(function (err, keyOrValue) {
        t.ifError(err)
        t.same(keyOrValue, mode === 'values' ? 'bar' : testKey)
      })
    })

    t.test(`unfixing sublevel.${mode}() Uint8Array keys (default implementation: ${def})`, function (t) {
      t.plan(4)

      const testKey = new Uint8Array([0, 255])
      const textEncoder = new TextEncoder()
      const prefix = textEncoder.encode('!test!')
      const prefixedKey = new Uint8Array(prefix.byteLength + testKey.byteLength)

      prefixedKey.set(prefix, 0)
      prefixedKey.set(testKey, prefix.byteLength)

      class MockIterator extends Ctor {
        _next (callback) {
          if (mode === 'iterator' || def) {
            this.nextTick(callback, null, prefixedKey, 'bar')
          } else if (mode === 'keys') {
            this.nextTick(callback, null, prefixedKey)
          } else {
            this.nextTick(callback, null, 'bar')
          }
        }
      }

      class MockLevel extends AbstractLevel {
        [privateMethod] (options) {
          t.is(options.keyEncoding, 'view')
          t.is(options.valueEncoding, 'utf8')
          return new MockIterator(this, options)
        }
      }

      const db = new MockLevel({ encodings: { buffer: true, view: true, utf8: true } })
      const sub = db.sublevel('test', { keyEncoding: 'view' })

      sub[publicMethod]().next(function (err, keyOrValue) {
        t.ifError(err)
        t.same(keyOrValue, mode === 'values' ? 'bar' : testKey)
      })
    })
  }

  t.end()
})

for (const chained of [false, true]) {
  for (const deferred of [false, true]) {
    test(`batch() with sublevel per operation (chained: ${chained}, deferred: ${deferred})`, async function (t) {
      t.plan(6)

      class MockLevel extends AbstractLevel {
        _batch (operations, options, callback) {
          t.same(operations, [
            {
              type: 'put',
              sublevel: null,
              key: '!1!a',
              value: '{"foo":123}',
              keyEncoding: 'utf8',
              valueEncoding: 'utf8'
            },
            {
              type: 'put',
              sublevel: null,
              key: '!2!a-y',
              value: '[object Object]',
              keyEncoding: 'utf8',
              valueEncoding: 'utf8'
            },
            {
              type: 'put',
              sublevel: null,
              key: '!1!b',
              value: '[object Object]',
              keyEncoding: 'utf8',
              valueEncoding: 'utf8'
            },
            {
              type: 'put',
              sublevel: null,
              key: '!2!b',
              value: 'b',
              keyEncoding: 'utf8',
              valueEncoding: 'utf8'
            },
            {
              type: 'del',
              sublevel: null,
              key: '!2!c1',
              keyEncoding: 'utf8'
            },
            {
              type: 'del',
              sublevel: null,
              key: '!2!c2-y',
              keyEncoding: 'utf8'
            },
            {
              type: 'del',
              key: 'd-x',
              keyEncoding: 'utf8'
            }
          ])
          t.same(options, {})
          nextTick(callback)
        }
      }

      const db = new MockLevel({ encodings: { utf8: true } }, {
        keyEncoding: {
          encode: (key) => key + '-x',
          decode: (key) => key.slice(0, -2),
          name: 'x',
          format: 'utf8'
        }
      })

      const sub1 = db.sublevel('1', { valueEncoding: 'json' })
      const sub2 = db.sublevel('2', {
        keyEncoding: {
          encode: (key) => key + '-y',
          decode: (key) => key.slice(0, -2),
          name: 'y',
          format: 'utf8'
        }
      })

      if (!deferred) await sub1.open()

      t.is(sub1.keyEncoding().name, 'utf8')
      t.is(sub1.valueEncoding().name, 'json')
      t.is(sub2.keyEncoding().name, 'y')
      t.is(sub2.valueEncoding().name, 'utf8')

      if (chained) {
        await db.batch()
          // keyEncoding: utf8 (sublevel), valueEncoding: json (sublevel)
          .put('a', { foo: 123 }, { sublevel: sub1 })

          // keyEncoding: y (sublevel), valueEncoding: utf8 (sublevel)
          .put('a', { foo: 123 }, { sublevel: sub2 })

          // keyEncoding: utf8 (sublevel), valueEncoding: utf8 (operation)
          .put('b', { foo: 123 }, { sublevel: sub1, valueEncoding: 'utf8' })

          // keyEncoding: utf8 (operation), valueEncoding: utf8 (sublevel)
          .put('b', 'b', { sublevel: sub2, keyEncoding: 'utf8' })

          // keyEncoding: utf8 (operation)
          .del('c1', { sublevel: sub2, keyEncoding: 'utf8' })

          // keyEncoding: y (sublevel)
          .del('c2', { sublevel: sub2 })

          // keyEncoding: x (db). Should not affect sublevels.
          .del('d')
          .write()
      } else {
        await db.batch([
          { type: 'put', sublevel: sub1, key: 'a', value: { foo: 123 } },
          { type: 'put', sublevel: sub2, key: 'a', value: { foo: 123 } },
          { type: 'put', sublevel: sub1, key: 'b', value: { foo: 123 }, valueEncoding: 'utf8' },
          { type: 'put', sublevel: sub2, key: 'b', value: 'b', keyEncoding: 'utf8' },
          { type: 'del', key: 'c1', sublevel: sub2, keyEncoding: 'utf8' },
          { type: 'del', key: 'c2', sublevel: sub2 },
          { type: 'del', key: 'd' }
        ])
      }
    })
  }
}
