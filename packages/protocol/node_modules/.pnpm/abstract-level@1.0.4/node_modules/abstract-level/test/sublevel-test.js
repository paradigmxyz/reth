'use strict'

const { Buffer } = require('buffer')

exports.all = function (test, testCommon) {
  for (const deferred of [false, true]) {
    // NOTE: adapted from subleveldown
    test(`sublevel.clear() (deferred: ${deferred})`, async function (t) {
      const db = testCommon.factory()
      const sub1 = db.sublevel('1')
      const sub2 = db.sublevel('2')

      if (!deferred) await sub1.open()
      if (!deferred) await sub2.open()

      await populate([sub1, sub2], ['a', 'b'])
      await verify(['!1!a', '!1!b', '!2!a', '!2!b'])

      await clear([sub1], {})
      await verify(['!2!a', '!2!b'])

      await populate([sub1], ['a', 'b'])
      await clear([sub2], { lt: 'b' })
      await verify(['!1!a', '!1!b', '!2!b'])
      await db.close()

      async function populate (subs, items) {
        return Promise.all(subs.map(sub => {
          return sub.batch(items.map(function (item) {
            return { type: 'put', key: item, value: item }
          }))
        }))
      }

      async function clear (subs, opts) {
        return Promise.all(subs.map(sub => {
          return sub.clear(opts)
        }))
      }

      async function verify (expected) {
        const keys = await db.keys().all()
        t.same(keys, expected)
      }
    })
  }

  for (const deferred of [false, true]) {
    for (const keyEncoding of ['buffer', 'view']) {
      if (!testCommon.supports.encodings[keyEncoding]) return

      // NOTE: adapted from subleveldown. See https://github.com/Level/subleveldown/issues/87
      test(`iterate sublevel keys with bytes above 196 (${keyEncoding}, deferred: ${deferred})`, async function (t) {
        const db = testCommon.factory()
        const sub1 = db.sublevel('a', { keyEncoding })
        const sub2 = db.sublevel('b', { keyEncoding })
        const length = (db) => db.keys().all().then(x => x.length)

        if (!deferred) await sub1.open()
        if (!deferred) await sub2.open()

        const batch1 = sub1.batch()
        const batch2 = sub2.batch()
        const keys = []

        for (let i = 0; i < 256; i++) {
          const key = keyEncoding === 'buffer' ? Buffer.from([i]) : new Uint8Array([i])
          keys.push(key)
          batch1.put(key, 'aa')
          batch2.put(key, 'bb')
        }

        await Promise.all([batch1.write(), batch2.write()])

        const entries1 = await sub1.iterator().all()
        const entries2 = await sub2.iterator().all()

        t.is(entries1.length, 256, 'sub1 yielded all entries')
        t.is(entries2.length, 256, 'sub2 yielded all entries')
        t.ok(entries1.every(x => x[1] === 'aa'))
        t.ok(entries2.every(x => x[1] === 'bb'))

        const many1 = await sub1.getMany(keys)
        const many2 = await sub2.getMany(keys)

        t.is(many1.length, 256, 'sub1 yielded all values')
        t.is(many2.length, 256, 'sub2 yielded all values')
        t.ok(many1.every(x => x === 'aa'))
        t.ok(many2.every(x => x === 'bb'))

        const singles1 = await Promise.all(keys.map(k => sub1.get(k)))
        const singles2 = await Promise.all(keys.map(k => sub2.get(k)))

        t.is(singles1.length, 256, 'sub1 yielded all values')
        t.is(singles2.length, 256, 'sub2 yielded all values')
        t.ok(singles1.every(x => x === 'aa'))
        t.ok(singles2.every(x => x === 'bb'))

        await sub1.clear()

        t.same(await length(sub1), 0, 'cleared sub1')
        t.same(await length(sub2), 256, 'did not clear sub2')

        await db.close()
      })
    }
  }
}
