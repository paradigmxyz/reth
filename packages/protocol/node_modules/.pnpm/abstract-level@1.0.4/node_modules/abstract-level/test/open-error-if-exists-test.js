'use strict'

exports.errorIfExists = function (test, testCommon) {
  test('test database open errorIfExists:true', function (t) {
    const db = testCommon.factory()

    db.open(function (err) {
      t.error(err)
      db.close(function (err) {
        t.error(err)

        let async = false

        db.open({ createIfMissing: false, errorIfExists: true }, function (err) {
          t.is(err && err.code, 'LEVEL_DATABASE_NOT_OPEN')
          t.ok(err && /exists/.test(err.cause && err.cause.message), 'error is about already existing')
          t.ok(async, 'callback is asynchronous')
          t.end()
        })

        async = true
      })
    })
  })
}

exports.all = function (test, testCommon) {
  exports.errorIfExists(test, testCommon)
}
