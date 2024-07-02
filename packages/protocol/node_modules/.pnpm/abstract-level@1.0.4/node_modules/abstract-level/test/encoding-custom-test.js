'use strict'

// NOTE: copied from levelup
exports.all = function (test, testCommon) {
  for (const deferred of [false, true]) {
    test(`custom encoding: simple-object values (deferred: ${deferred})`, function (t) {
      run(t, deferred, [
        { key: '0', value: 0 },
        { key: '1', value: 1 },
        { key: 'string', value: 'a string' },
        { key: 'true', value: true },
        { key: 'false', value: false }
      ])
    })

    test(`custom encoding: simple-object keys (deferred: ${deferred})`, function (t) {
      // Test keys that would be considered the same with default utf8 encoding.
      // Because String([1]) === String(1).
      run(t, deferred, [
        { value: '0', key: [1] },
        { value: '1', key: 1 },
        { value: 'string', key: 'a string' },
        { value: 'true', key: true },
        { value: 'false', key: false }
      ])
    })

    test(`custom encoding: complex-object values (deferred: ${deferred})`, function (t) {
      run(t, deferred, [{
        key: '0',
        value: {
          foo: 'bar',
          bar: [1, 2, 3],
          bang: { yes: true, no: false }
        }
      }])
    })

    test(`custom encoding: complex-object keys (deferred: ${deferred})`, function (t) {
      // Test keys that would be considered the same with default utf8 encoding.
      // Because String({}) === String({}) === '[object Object]'.
      run(t, deferred, [{
        value: '0',
        key: {
          foo: 'bar',
          bar: [1, 2, 3],
          bang: { yes: true, no: false }
        }
      }, {
        value: '1',
        key: {
          foo: 'different',
          bar: [1, 2, 3],
          bang: { yes: true, no: false }
        }
      }])
    })
  }

  function run (t, deferred, entries) {
    const customEncoding = {
      encode: JSON.stringify,
      decode: JSON.parse,
      format: 'utf8',
      type: 'custom'
    }

    const db = testCommon.factory({
      keyEncoding: customEncoding,
      valueEncoding: customEncoding
    })

    const operations = entries.map(entry => ({ type: 'put', ...entry }))
    const init = deferred ? (fn) => fn() : db.open.bind(db)

    init(function (err) {
      t.ifError(err, 'no init() error')

      db.batch(operations, function (err) {
        t.ifError(err, 'no batch() error')

        let pending = entries.length
        const next = () => {
          if (--pending === 0) {
            db.close(t.end.bind(t))
          }
        }

        for (const entry of entries) {
          db.get(entry.key, function (err, value) {
            t.ifError(err, 'no get() error')
            t.same(value, entry.value)
            next()
          })
        }
      })
    })
  }
}
