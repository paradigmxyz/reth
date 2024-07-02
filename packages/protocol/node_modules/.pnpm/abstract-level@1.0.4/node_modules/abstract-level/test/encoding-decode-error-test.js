'use strict'

let db
let keySequence = 0

const testKey = () => 'test' + (++keySequence)

exports.all = function (test, testCommon) {
  test('setup', async function (t) {
    db = testCommon.factory()
    return db.open()
  })

  // NOTE: adapted from encoding-down
  test('get() and getMany() forward decode error', function (t) {
    const key = testKey()
    const valueEncoding = {
      encode: (v) => v,
      decode: (v) => { throw new Error('decode error xyz') },
      format: 'utf8'
    }

    db.put(key, 'bar', { valueEncoding }, function (err) {
      t.ifError(err, 'no put() error')

      db.get(key, { valueEncoding }, function (err, value) {
        t.is(err && err.code, 'LEVEL_DECODE_ERROR')
        t.is(err && err.cause && err.cause.message, 'decode error xyz')
        t.is(value, undefined)

        db.getMany(['other-key', key], { valueEncoding }, function (err, values) {
          t.is(err && err.code, 'LEVEL_DECODE_ERROR')
          t.is(err && err.cause && err.cause.message, 'decode error xyz')
          t.is(values, undefined)
          t.end()
        })
      })
    })
  })

  // NOTE: adapted from encoding-down
  test('get() and getMany() yield encoding error if stored value is invalid', function (t) {
    const key = testKey()

    db.put(key, 'this {} is [] not : json', { valueEncoding: 'utf8' }, function (err) {
      t.ifError(err, 'no put() error')

      db.get(key, { valueEncoding: 'json' }, function (err) {
        t.is(err && err.code, 'LEVEL_DECODE_ERROR')
        t.is(err && err.cause.name, 'SyntaxError') // From JSON.parse()

        db.getMany(['other-key', key], { valueEncoding: 'json' }, function (err) {
          t.is(err && err.code, 'LEVEL_DECODE_ERROR')
          t.is(err && err.cause.name, 'SyntaxError') // From JSON.parse()
          t.end()
        })
      })
    })
  })

  test('teardown', async function (t) {
    return db.close()
  })
}
