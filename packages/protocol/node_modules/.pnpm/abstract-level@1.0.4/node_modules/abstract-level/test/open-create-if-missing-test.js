'use strict'

exports.createIfMissing = function (test, testCommon) {
  test('test database open createIfMissing:false', function (t) {
    const db = testCommon.factory()
    let async = false

    db.open({ createIfMissing: false }, function (err) {
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
      t.ok(err && /does not exist/.test(err.cause && err.cause.message), 'error is about dir not existing')
      t.ok(async, 'callback is asynchronous')
      t.end()
    })

    async = true
  })

  test('test database open createIfMissing:false via constructor', function (t) {
    const db = testCommon.factory({ createIfMissing: false })
    let async = false

    db.open(function (err) {
      t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
      t.ok(err && /does not exist/.test(err.cause && err.cause.message), 'error is about dir not existing')
      t.ok(async, 'callback is asynchronous')
      t.end()
    })

    async = true
  })
}

exports.all = function (test, testCommon) {
  exports.createIfMissing(test, testCommon)
}
