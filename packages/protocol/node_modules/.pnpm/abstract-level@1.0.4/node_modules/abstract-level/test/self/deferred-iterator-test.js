'use strict'

const test = require('tape')
const { DeferredIterator, DeferredKeyIterator, DeferredValueIterator } = require('../../lib/deferred-iterator')
const { AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('../..')
const { mockLevel } = require('../util')
const noop = () => {}
const identity = (v) => v

for (const mode of ['iterator', 'keys', 'values']) {
  const RealCtor = mode === 'iterator' ? AbstractIterator : mode === 'keys' ? AbstractKeyIterator : AbstractValueIterator
  const DeferredCtor = mode === 'iterator' ? DeferredIterator : mode === 'keys' ? DeferredKeyIterator : DeferredValueIterator
  const nextArgs = mode === 'iterator' ? ['key', 'value'] : mode === 'keys' ? ['key'] : ['value']
  const privateMethod = '_' + mode
  const publicMethod = mode

  // NOTE: adapted from deferred-leveldown
  test(`deferred ${mode}()`, function (t) {
    t.plan(8)

    const keyEncoding = {
      format: 'utf8',
      encode (key) {
        t.is(key, 'foo', 'encoding got key')
        return key.toUpperCase()
      },
      decode: identity
    }

    class MockIterator extends RealCtor {
      _next (cb) {
        this.nextTick(cb, null, ...nextArgs)
      }

      _close (cb) {
        this.nextTick(cb)
      }
    }

    const db = mockLevel({
      [privateMethod]: function (options) {
        t.is(options.gt, 'FOO', 'got encoded range option')
        return new MockIterator(this, options)
      },
      _open: function (options, callback) {
        t.pass('opened')
        this.nextTick(callback)
      }
    }, { encodings: { utf8: true } }, {
      keyEncoding
    })

    const it = db[publicMethod]({ gt: 'foo' })
    t.ok(it instanceof DeferredCtor, 'is deferred')

    let nextFirst = false

    it.next(function (err, ...rest) {
      nextFirst = true
      t.error(err, 'no next() error')
      t.same(rest, nextArgs)
    })

    it.close(function (err) {
      t.error(err, 'no close() error')
      t.ok(nextFirst)
    })
  })

  // NOTE: adapted from deferred-leveldown
  test(`deferred ${mode}(): non-deferred operations`, function (t) {
    t.plan(6)

    class MockIterator extends RealCtor {
      _seek (target) {
        t.is(target, '123')
      }

      _next (cb) {
        this.nextTick(cb, null, ...nextArgs)
      }
    }

    const db = mockLevel({
      [privateMethod]: function (options) {
        return new MockIterator(this, options)
      }
    })

    db.open(function (err) {
      t.error(err, 'no open() error')

      it.seek(123)
      it.next(function (err, ...rest) {
        t.error(err, 'no next() error')
        t.same(rest, nextArgs)

        it.close(function (err) {
          t.error(err, 'no close() error')
        })
      })
    })

    const it = db[publicMethod]({ gt: 'foo' })
    t.ok(it instanceof DeferredCtor)
  })

  // NOTE: adapted from deferred-leveldown
  test(`deferred ${mode}(): iterators are created in order`, function (t) {
    t.plan(6)

    const order1 = []
    const order2 = []

    class MockIterator extends RealCtor {}

    function db (order) {
      return mockLevel({
        [privateMethod]: function (options) {
          order.push('iterator created')
          return new MockIterator(this, options)
        },
        _put: function (key, value, options, callback) {
          order.push('put')
        },
        _open: function (options, callback) {
          this.nextTick(callback)
        }
      })
    }

    const db1 = db(order1)
    const db2 = db(order2)

    db1.open(function (err) {
      t.error(err, 'no error')
      t.same(order1, ['iterator created', 'put'])
    })

    db2.open(function (err) {
      t.error(err, 'no error')
      t.same(order2, ['put', 'iterator created'])
    })

    t.ok(db1[publicMethod]() instanceof DeferredCtor)
    db1.put('key', 'value', noop)

    db2.put('key', 'value', noop)
    t.ok(db2[publicMethod]() instanceof DeferredCtor)
  })

  for (const method of ['next', 'nextv', 'all']) {
    test(`deferred ${mode}(): closed upon failed open, verified by ${method}()`, function (t) {
      t.plan(5)

      const db = mockLevel({
        _open (options, callback) {
          t.pass('opening')
          this.nextTick(callback, new Error('_open error'))
        },
        _iterator () {
          t.fail('should not be called')
        },
        [privateMethod] () {
          t.fail('should not be called')
        }
      })

      const it = db[publicMethod]()
      t.ok(it instanceof DeferredCtor)

      const original = it._close
      it._close = function (...args) {
        t.pass('closed')
        return original.call(this, ...args)
      }

      verifyClosed(t, it, method, () => {})
    })

    test(`deferred ${mode}(): deferred and real iterators are closed on db.close(), verified by ${method}()`, function (t) {
      t.plan(10)

      class MockIterator extends RealCtor {
        _close (callback) {
          t.pass('closed')
          this.nextTick(callback)
        }
      }

      const db = mockLevel({
        [privateMethod] (options) {
          return new MockIterator(this, options)
        }
      })

      const it = db[publicMethod]()
      t.ok(it instanceof DeferredCtor)

      const original = it._close
      it._close = function (...args) {
        t.pass('closed')
        return original.call(this, ...args)
      }

      db.close(function (err) {
        t.ifError(err, 'no close() error')

        verifyClosed(t, it, method, function () {
          db.open(function (err) {
            t.ifError(err, 'no open() error')

            // Should still be closed
            verifyClosed(t, it, method, function () {
              db.close(t.ifError.bind(t))
            })
          })
        })
      })
    })
  }

  test(`deferred ${mode}(): deferred and real iterators are detached on db.close()`, function (t) {
    t.plan(4)

    class MockIterator extends RealCtor {}

    let real
    const db = mockLevel({
      [privateMethod] (options) {
        real = new MockIterator(this, options)
        return real
      }
    })

    const it = db[publicMethod]()
    t.ok(it instanceof DeferredCtor)

    db.close(function (err) {
      t.ifError(err, 'no close() error')

      db.open(function (err) {
        t.ifError(err, 'no open() error')

        it.close = real.close = it._close = real._close = function () {
          t.fail('should not be called')
        }

        db.close(t.ifError.bind(t))
      })
    })
  })

  test(`deferred ${mode}(): defers underlying close()`, function (t) {
    t.plan(3)

    class MockIterator extends RealCtor {
      _close (callback) {
        order.push('_close')
        this.nextTick(callback)
      }
    }

    const order = []
    const db = mockLevel({
      _open (options, callback) {
        order.push('_open')
        this.nextTick(callback)
      },
      [privateMethod] (options) {
        order.push(privateMethod)
        return new MockIterator(this, options)
      }
    })

    const it = db[publicMethod]()
    t.ok(it instanceof DeferredCtor)

    it.close(function (err) {
      t.ifError(err, 'no close() error')
      t.same(order, ['_open', privateMethod, '_close'])
    })
  })

  const verifyClosed = function (t, it, method, cb) {
    const requiredArgs = method === 'nextv' ? [10] : []

    it[method](...requiredArgs, function (err) {
      t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN', `correct error on first ${method}()`)

      // Should account for userland code that ignores errors
      it[method](...requiredArgs, function (err) {
        t.is(err && err.code, 'LEVEL_ITERATOR_NOT_OPEN', `correct error on second ${method}()`)
        cb()
      })
    })
  }
}
