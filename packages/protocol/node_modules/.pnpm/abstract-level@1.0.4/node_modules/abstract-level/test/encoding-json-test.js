'use strict'

// NOTE: copied from levelup
exports.all = function (test, testCommon) {
  for (const deferred of [false, true]) {
    test(`json encoding: simple-object values (deferred: ${deferred})`, function (t) {
      run(t, deferred, [
        { key: '0', value: 0 },
        { key: '1', value: 1 },
        { key: '2', value: 'a string' },
        { key: '3', value: true },
        { key: '4', value: false }
      ])
    })

    test(`json encoding: simple-object keys (deferred: ${deferred})`, function (t) {
      run(t, deferred, [
        { value: 'string', key: 'a string' },
        { value: '0', key: 0 },
        { value: '1', key: 1 },
        { value: 'false', key: false },
        { value: 'true', key: true }
      ])
    })

    test(`json encoding: complex-object values (deferred: ${deferred})`, function (t) {
      run(t, deferred, [{
        key: '0',
        value: {
          foo: 'bar',
          bar: [1, 2, 3],
          bang: { yes: true, no: false }
        }
      }])
    })

    test(`json encoding: complex-object keys (deferred: ${deferred})`, function (t) {
      run(t, deferred, [{
        value: '0',
        key: {
          foo: 'bar',
          bar: [1, 2, 3],
          bang: { yes: true, no: false }
        }
      }])
    })
  }

  function run (t, deferred, entries) {
    const db = testCommon.factory({ keyEncoding: 'json', valueEncoding: 'json' })
    const operations = entries.map(entry => ({ type: 'put', ...entry }))
    const init = deferred ? (fn) => fn() : db.open.bind(db)

    init(function (err) {
      t.ifError(err, 'no init() error')

      db.batch(operations, function (err) {
        t.ifError(err, 'no batch() error')

        let pending = entries.length + 1
        const next = () => {
          if (--pending === 0) db.close(t.end.bind(t))
        }

        testGet(next)
        testIterator(next)
      })
    })

    function testGet (next) {
      for (const entry of entries) {
        db.get(entry.key, function (err, value) {
          t.ifError(err, 'no get() error')
          t.same(value, entry.value)
          next()
        })
      }
    }

    function testIterator (next) {
      db.iterator().all(function (err, result) {
        t.ifError(err, 'no all() error')
        t.same(result, entries.map(kv => [kv.key, kv.value]))
        next()
      })
    }
  }
}
