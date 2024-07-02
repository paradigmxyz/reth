'use strict'

module.exports = function (test, testCommon) {
  test('testCommon.factory() returns valid database', function (t) {
    t.plan(6)

    const db = testCommon.factory()
    const kEvent = Symbol('event')

    // Avoid instanceof, for levelup compatibility tests
    t.is(typeof db, 'object', 'is an object')
    t.isNot(db, null, 'is not null')
    t.is(typeof db.open, 'function', 'has open() method')
    t.is(typeof db.on, 'function', 'has on() method')
    t.is(typeof db.emit, 'function', 'has emit() method')

    db.once(kEvent, (v) => t.is(v, 'foo', 'got event'))
    db.emit(kEvent, 'foo')
  })

  test('testCommon.factory() returns a unique database', function (t) {
    const db1 = testCommon.factory()
    const db2 = testCommon.factory()

    t.isNot(db1, db2, 'unique instances')

    function close () {
      db1.close(function (err) {
        t.error(err, 'no error while closing db1')
        db2.close(function (err) {
          t.error(err, 'no error while closing db2')
          t.end()
        })
      })
    }

    db1.open(function (err) {
      t.error(err, 'no error while opening db1')
      db2.open(function (err) {
        t.error(err, 'no error while opening db2')
        db1.put('key', 'value', function (err) {
          t.error(err, 'put key in db1')
          db2.get('key', function (err, value) {
            t.ok(err, 'db2 should be empty')
            t.is(value, undefined, 'db2 should be empty')
            close()
          })
        })
      })
    })
  })
}
