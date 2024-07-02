'use strict'

const { Buffer } = require('buffer')
const identity = (v) => v

exports.all = function (test, testCommon) {
  exports.sequence(test, testCommon)
  exports.seek(test, testCommon)
}

exports.sequence = function (test, testCommon) {
  for (const deferred of [false, true]) {
    for (const mode of ['iterator', 'keys', 'values']) {
      test(`${mode}().seek() throws if next() has not completed (deferred: ${deferred})`, async function (t) {
        const db = testCommon.factory()
        if (!deferred) await db.open()

        const it = db[mode]()
        const promise = it.next()

        t.throws(() => it.seek('two'), (err) => err.code === 'LEVEL_ITERATOR_BUSY')

        await promise
        await db.close()
      })

      test(`${mode}().seek() does not throw after close() (deferred: ${deferred})`, async function (t) {
        const db = testCommon.factory()
        if (!deferred) await db.open()

        const it = db[mode]()
        await it.close()

        t.doesNotThrow(() => it.seek('two'))

        await db.close()
      })
    }
  }
}

exports.seek = function (test, testCommon) {
  const testData = () => [
    { type: 'put', key: 'one', value: '1' },
    { type: 'put', key: 'two', value: '2' },
    { type: 'put', key: 'three', value: '3' }
  ]

  for (const mode of ['iterator', 'keys', 'values']) {
    const mapEntry = mode === 'iterator' ? e => e : mode === 'keys' ? e => e[0] : e => e[1]

    test(`${mode}().seek() to string target`, async function (t) {
      const db = testCommon.factory()
      await db.batch(testData())
      const it = db[mode]()

      it.seek('two')

      t.same(await it.next(), mapEntry(['two', '2']), 'match')
      t.same(await it.next(), undefined, 'end of iterator')

      return db.close()
    })

    if (testCommon.supports.encodings.buffer) {
      // TODO: make this test meaningful, with bytes outside the utf8 range
      test(`${mode}().seek() to buffer target`, async function (t) {
        const db = testCommon.factory()
        await db.batch(testData())
        const it = db[mode]({ keyEncoding: 'buffer' })

        it.seek(Buffer.from('two'))

        t.same(await it.next(), mapEntry([Buffer.from('two'), '2']), 'match')
        t.same(await it.next(), undefined, 'end of iterator')

        return db.close()
      })
    }

    test(`${mode}().seek() to target with custom encoding`, async function (t) {
      const db = testCommon.factory()
      await db.batch(testData())
      const it = db[mode]()
      const keyEncoding = { encode: () => 'two', decode: identity, format: 'utf8' }

      it.seek('xyz', { keyEncoding })

      t.same(await it.next(), mapEntry(['two', '2']), 'match')
      t.same(await it.next(), undefined, 'end of iterator')

      return db.close()
    })

    test(`${mode}().seek() on reverse iterator`, async function (t) {
      const db = testCommon.factory()
      await db.batch(testData())
      const it = db[mode]({ reverse: true, limit: 1 })

      // Should land on key equal to or smaller than 'three!' which is 'three'
      it.seek('three!')

      t.same(await it.next(), mapEntry(['three', '3']), 'match')
      t.same(await it.next(), undefined, 'end of iterator')

      return db.close()
    })

    test(`${mode}().seek() to out of range target`, async function (t) {
      const db = testCommon.factory()
      await db.batch(testData())
      const it = db[mode]()

      it.seek('zzz')
      t.same(await it.next(), undefined, 'end of iterator')

      return db.close()
    })

    test(`${mode}().seek() on reverse iterator to out of range target`, async function (t) {
      const db = testCommon.factory()
      await db.batch(testData())
      const it = db[mode]({ reverse: true })

      it.seek('zzz')

      t.same(await it.next(), mapEntry(['two', '2']), 'match')
      t.same(await it.next(), mapEntry(['three', '3']), 'match')
      t.same(await it.next(), mapEntry(['one', '1']), 'match')
      t.same(await it.next(), undefined, 'end of iterator')

      return db.close()
    })

    if (testCommon.supports.snapshots) {
      for (const reverse of [false, true]) {
        for (const deferred of [false, true]) {
          test(`${mode}().seek() respects snapshot (reverse: ${reverse}, deferred: ${deferred})`, async function (t) {
            const db = testCommon.factory()
            if (!deferred) await db.open()

            const it = db[mode]({ reverse })

            // Add entry after having created the iterator (and its snapshot)
            await db.put('a', 'a')

            // Seeking should not create a new snapshot, which'd include the new entry
            it.seek('a')
            t.same(await it.next(), undefined)

            return db.close()
          })
        }
      }
    }

    test(`${mode}().seek() respects range`, function (t) {
      const db = testCommon.factory()

      db.open(function (err) {
        t.error(err, 'no error from open()')

        // Can't use Array.fill() because IE
        const ops = []

        for (let i = 0; i < 10; i++) {
          ops.push({ type: 'put', key: String(i), value: String(i) })
        }

        db.batch(ops, function (err) {
          t.error(err, 'no error from batch()')

          let pending = 0

          expect({ gt: '5' }, '4', undefined)
          expect({ gt: '5' }, '5', undefined)
          expect({ gt: '5' }, '6', '6')

          expect({ gte: '5' }, '4', undefined)
          expect({ gte: '5' }, '5', '5')
          expect({ gte: '5' }, '6', '6')

          // The gte option should take precedence over gt.
          expect({ gte: '5', gt: '7' }, '4', undefined)
          expect({ gte: '5', gt: '7' }, '5', '5')
          expect({ gte: '5', gt: '7' }, '6', '6')
          expect({ gte: '5', gt: '3' }, '4', undefined)
          expect({ gte: '5', gt: '3' }, '5', '5')
          expect({ gte: '5', gt: '3' }, '6', '6')

          expect({ lt: '5' }, '4', '4')
          expect({ lt: '5' }, '5', undefined)
          expect({ lt: '5' }, '6', undefined)

          expect({ lte: '5' }, '4', '4')
          expect({ lte: '5' }, '5', '5')
          expect({ lte: '5' }, '6', undefined)

          // The lte option should take precedence over lt.
          expect({ lte: '5', lt: '3' }, '4', '4')
          expect({ lte: '5', lt: '3' }, '5', '5')
          expect({ lte: '5', lt: '3' }, '6', undefined)
          expect({ lte: '5', lt: '7' }, '4', '4')
          expect({ lte: '5', lt: '7' }, '5', '5')
          expect({ lte: '5', lt: '7' }, '6', undefined)

          expect({ lt: '5', reverse: true }, '4', '4')
          expect({ lt: '5', reverse: true }, '5', undefined)
          expect({ lt: '5', reverse: true }, '6', undefined)

          expect({ lte: '5', reverse: true }, '4', '4')
          expect({ lte: '5', reverse: true }, '5', '5')
          expect({ lte: '5', reverse: true }, '6', undefined)

          expect({ gt: '5', reverse: true }, '4', undefined)
          expect({ gt: '5', reverse: true }, '5', undefined)
          expect({ gt: '5', reverse: true }, '6', '6')

          expect({ gte: '5', reverse: true }, '4', undefined)
          expect({ gte: '5', reverse: true }, '5', '5')
          expect({ gte: '5', reverse: true }, '6', '6')

          expect({ gt: '7', lt: '8' }, '7', undefined)
          expect({ gte: '7', lt: '8' }, '7', '7')
          expect({ gte: '7', lt: '8' }, '8', undefined)
          expect({ gt: '7', lte: '8' }, '8', '8')

          function expect (range, target, expected) {
            pending++
            const ite = db[mode](range)

            ite.seek(target)
            ite.next(function (err, item) {
              t.error(err, 'no error from next()')

              const json = JSON.stringify(range)
              const msg = 'seek(' + target + ') on ' + json + ' yields ' + expected

              // Either a key or value depending on mode
              t.is(item, expected, msg)

              ite.close(function (err) {
                t.error(err, 'no error from close()')
                if (!--pending) db.close(t.end.bind(t))
              })
            })
          }
        })
      })
    })
  }
}
