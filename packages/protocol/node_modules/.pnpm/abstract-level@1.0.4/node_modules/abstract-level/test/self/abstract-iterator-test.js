'use strict'

const test = require('tape')
const { AbstractLevel, AbstractIterator, AbstractKeyIterator, AbstractValueIterator } = require('../..')

const testCommon = require('../common')({
  test,
  factory: function () {
    return new AbstractLevel({ encodings: { utf8: true } })
  }
})

for (const Ctor of [AbstractIterator, AbstractKeyIterator, AbstractValueIterator]) {
  test(`test ${Ctor.name} extensibility`, function (t) {
    const Test = class TestIterator extends Ctor {}
    const db = testCommon.factory()
    const test = new Test(db, {})
    t.ok(test.db === db, 'instance has db reference')
    t.end()
  })

  test(`${Ctor.name} throws on invalid db argument`, function (t) {
    t.plan(4 * 2)

    for (const args of [[], [null], [undefined], 'foo']) {
      const hint = args[0] === null ? 'null' : typeof args[0]

      try {
        // eslint-disable-next-line no-new
        new Ctor(...args)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'The first argument must be an abstract-level database, received ' + hint)
      }
    }
  })

  test(`${Ctor.name} throws on invalid options argument`, function (t) {
    t.plan(4 * 2)

    for (const args of [[], [null], [undefined], 'foo']) {
      try {
        // eslint-disable-next-line no-new
        new Ctor({}, ...args)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'The second argument must be an options object')
      }
    }
  })

  test(`${Ctor.name}.next() extensibility`, async function (t) {
    t.plan(3)

    class TestIterator extends Ctor {
      _next (callback) {
        t.is(this, it, 'thisArg on _next() was correct')
        t.is(arguments.length, 1, 'got one argument')
        t.is(typeof callback, 'function', 'got a callback function')
        this.nextTick(callback)
      }
    }

    const db = testCommon.factory()
    await db.open()
    const it = new TestIterator(db, {})
    await it.next()
    await db.close()
  })

  test(`${Ctor.name}.next() throws on invalid callback argument`, async function (t) {
    t.plan(3 * 2)

    const db = testCommon.factory()
    await db.open()

    for (const invalid of [{}, null, 'foo']) {
      const it = new Ctor(db, {})

      try {
        it.next(invalid)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'Callback must be a function')
      }
    }
  })

  test(`${Ctor.name}.nextv() extensibility`, async function (t) {
    t.plan(5 * 2)

    class TestIterator extends Ctor {
      _nextv (size, options, callback) {
        t.is(this, it, 'thisArg on _nextv() was correct')
        t.is(arguments.length, 3, 'got 3 arguments')
        t.is(size, 100)
        t.same(options, {}, 'empty options')
        t.is(typeof callback, 'function', 'got a callback function')
        this.nextTick(callback, null, [])
      }
    }

    const db = testCommon.factory()
    await db.open()
    const it = new TestIterator(db, {})
    await it.nextv(100)
    await it.nextv(100, {})
    await db.close()
  })

  test(`${Ctor.name}.nextv() extensibility (options)`, async function (t) {
    t.plan(2)

    class TestIterator extends Ctor {
      _nextv (size, options, callback) {
        t.is(size, 100)
        t.same(options, { foo: 123 }, 'got options')
        this.nextTick(callback, null, [])
      }
    }

    const db = testCommon.factory()
    await db.open()
    const it = new TestIterator(db, {})
    await it.nextv(100, { foo: 123 })
    await db.close()
  })

  test(`${Ctor.name}.nextv() throws on invalid callback argument`, async function (t) {
    t.plan(3 * 2)

    const db = testCommon.factory()
    await db.open()

    for (const invalid of [{}, null, 'foo']) {
      const it = new Ctor(db, {})

      try {
        it.nextv(100, {}, invalid)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'Callback must be a function')
      }
    }
  })

  test(`${Ctor.name}.all() extensibility`, async function (t) {
    t.plan(2 * 4)

    for (const args of [[], [{}]]) {
      class TestIterator extends Ctor {
        _all (options, callback) {
          t.is(this, it, 'thisArg on _all() was correct')
          t.is(arguments.length, 2, 'got 2 arguments')
          t.same(options, {}, 'empty options')
          t.is(typeof callback, 'function', 'got a callback function')
          this.nextTick(callback, null, [])
        }
      }

      const db = testCommon.factory()
      await db.open()
      const it = new TestIterator(db, {})
      await it.all(...args)
      await db.close()
    }
  })

  test(`${Ctor.name}.all() extensibility (options)`, async function (t) {
    t.plan(1)

    class TestIterator extends Ctor {
      _all (options, callback) {
        t.same(options, { foo: 123 }, 'got options')
        this.nextTick(callback, null, [])
      }
    }

    const db = testCommon.factory()
    await db.open()
    const it = new TestIterator(db, {})
    await it.all({ foo: 123 })
    await db.close()
  })

  test(`${Ctor.name}.all() throws on invalid callback argument`, async function (t) {
    t.plan(3 * 2)

    const db = testCommon.factory()
    await db.open()

    for (const invalid of [{}, null, 'foo']) {
      const it = new Ctor(db, {})

      try {
        it.all({}, invalid)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'Callback must be a function')
      }
    }
  })

  test(`${Ctor.name}.close() extensibility`, async function (t) {
    t.plan(3)

    class TestIterator extends Ctor {
      _close (callback) {
        t.is(this, it, 'thisArg on _close() was correct')
        t.is(arguments.length, 1, 'got one argument')
        t.is(typeof callback, 'function', 'got a callback function')
        this.nextTick(callback)
      }
    }

    const db = testCommon.factory()
    await db.open()
    const it = new TestIterator(db, {})
    await it.close()
    await db.close()
  })

  test(`${Ctor.name}.close() throws on invalid callback argument`, async function (t) {
    t.plan(3 * 2)

    const db = testCommon.factory()
    await db.open()

    for (const invalid of [{}, null, 'foo']) {
      const it = new Ctor(db, {})

      try {
        it.close(invalid)
      } catch (err) {
        t.is(err.name, 'TypeError')
        t.is(err.message, 'Callback must be a function')
      }
    }
  })
}

test('AbstractIterator throws when accessing legacy properties', async function (t) {
  t.plan(3 * 2)

  const db = testCommon.factory()
  await db.open()
  const it = new AbstractIterator(db, {})

  for (const k of ['_ended property', '_nexting property', '_end method']) {
    try {
      // eslint-disable-next-line no-unused-expressions
      it[k.split(' ')[0]]
    } catch (err) {
      t.is(err.code, 'LEVEL_LEGACY')
    }

    try {
      it[k.split(' ')[0]] = 123
    } catch (err) {
      t.is(err.code, 'LEVEL_LEGACY')
    }
  }
})
