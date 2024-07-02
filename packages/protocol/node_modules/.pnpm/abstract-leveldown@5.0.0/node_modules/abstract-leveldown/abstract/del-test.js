var db
var leveldown
var testCommon
var verifyNotFoundError = require('./util').verifyNotFoundError

module.exports.setUp = function (_leveldown, test, _testCommon) {
  test('setUp common', _testCommon.setUp)
  test('setUp db', function (t) {
    leveldown = _leveldown
    testCommon = _testCommon
    db = leveldown(testCommon.location())
    db.open(t.end.bind(t))
  })
}

module.exports.args = function (test) {
  test('test argument-less del() throws', function (t) {
    t.throws(
      db.del.bind(db)
      , { name: 'Error', message: 'del() requires a callback argument' }
      , 'no-arg del() throws'
    )
    t.end()
  })

  test('test callback-less, 1-arg, del() throws', function (t) {
    t.throws(
      db.del.bind(db, 'foo')
      , { name: 'Error', message: 'del() requires a callback argument' }
      , 'callback-less, 1-arg del() throws'
    )
    t.end()
  })

  test('test callback-less, 3-arg, del() throws', function (t) {
    t.throws(
      db.del.bind(db, 'foo', {})
      , { name: 'Error', message: 'del() requires a callback argument' }
      , 'callback-less, 2-arg del() throws'
    )
    t.end()
  })

  test('test custom _serialize*', function (t) {
    t.plan(3)
    var db = leveldown(testCommon.location())
    db._serializeKey = function (data) { return data }
    db._del = function (key, options, callback) {
      t.deepEqual(key, { foo: 'bar' })
      process.nextTick(callback)
    }
    db.open(function () {
      db.del({ foo: 'bar' }, function (err) {
        t.error(err)
        db.close(t.error.bind(t))
      })
    })
  })
}

module.exports.del = function (test) {
  test('test simple del()', function (t) {
    db.put('foo', 'bar', function (err) {
      t.error(err)
      db.del('foo', function (err) {
        t.error(err)
        db.get('foo', function (err, value) {
          t.ok(err, 'entry propertly deleted')
          t.ok(typeof value === 'undefined', 'value is undefined')
          t.ok(verifyNotFoundError(err), 'NotFound error')
          t.end()
        })
      })
    })
  })

  test('test del on non-existent key', function (t) {
    db.del('blargh', function (err) {
      t.error(err)
      t.end()
    })
  })
}

module.exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(testCommon.tearDown.bind(null, t))
  })
}

module.exports.all = function (leveldown, test, testCommon) {
  testCommon = testCommon || require('../testCommon')
  module.exports.setUp(leveldown, test, testCommon)
  module.exports.args(test)
  module.exports.del(test)
  module.exports.tearDown(test, testCommon)
}
