'use strict'

const { verifyNotFoundError } = require('./util')
const { Buffer } = require('buffer')

let db

function makeTest (test, type, key, value, expectedResult) {
  const hasExpectedResult = arguments.length === 5
  test('test put()/get()/del() with ' + type, function (t) {
    db.put(key, value, function (err) {
      t.error(err)
      db.get(key, function (err, _value) {
        t.error(err, 'no error, has key/value for `' + type + '`')

        let result = _value

        if (hasExpectedResult) {
          t.equal(result.toString(), expectedResult)
        } else {
          if (result != null) { result = _value.toString() }
          if (value != null) { value = value.toString() }
          t.equals(result, value)
        }
        db.del(key, function (err) {
          t.error(err, 'no error, deleted key/value for `' + type + '`')

          let async = false

          db.get(key, function (err, value) {
            t.ok(err, 'entry properly deleted')
            t.ok(verifyNotFoundError(err), 'correct error')
            t.is(value, undefined, 'value is undefined')
            t.ok(async, 'callback is asynchronous')
            t.end()
          })

          async = true
        })
      })
    })
  })
}

exports.setUp = function (test, testCommon) {
  test('setUp db', function (t) {
    db = testCommon.factory()
    db.open(t.end.bind(t))
  })
}

exports.nonErrorKeys = function (test, testCommon) {
  // valid falsey keys
  makeTest(test, '`0` key', 0, 'foo 0')
  makeTest(test, 'empty string key', 0, 'foo')

  // standard String key
  makeTest(
    test
    , 'long String key'
    , 'some long string that I\'m using as a key for this unit test, cross your fingers human, we\'re going in!'
    , 'foo'
  )

  if (testCommon.supports.encodings.buffer) {
    makeTest(test, 'Buffer key', Buffer.from('0080c0ff', 'hex'), 'foo')
    makeTest(test, 'empty Buffer key', Buffer.alloc(0), 'foo')
  }

  // non-empty Array as a value
  makeTest(test, 'Array value', 'foo', [1, 2, 3, 4])
}

exports.nonErrorValues = function (test, testCommon) {
  // valid falsey values
  makeTest(test, '`false` value', 'foo false', false)
  makeTest(test, '`0` value', 'foo 0', 0)
  makeTest(test, '`NaN` value', 'foo NaN', NaN)

  // all of the following result in an empty-string value:
  makeTest(test, 'empty String value', 'foo', '', '')
  makeTest(test, 'empty Buffer value', 'foo', Buffer.alloc(0), '')
  makeTest(test, 'empty Array value', 'foo', [], '')

  // String value
  makeTest(
    test
    , 'long String value'
    , 'foo'
    , 'some long string that I\'m using as a key for this unit test, cross your fingers human, we\'re going in!'
  )

  // Buffer value
  if (testCommon.supports.encodings.buffer) {
    makeTest(test, 'Buffer value', 'foo', Buffer.from('0080c0ff', 'hex'))
  }

  // non-empty Array as a key
  makeTest(test, 'Array key', [1, 2, 3, 4], 'foo')
}

exports.tearDown = function (test, testCommon) {
  test('tearDown', function (t) {
    db.close(t.end.bind(t))
  })
}

exports.all = function (test, testCommon) {
  exports.setUp(test, testCommon)
  exports.nonErrorKeys(test, testCommon)
  exports.nonErrorValues(test, testCommon)
  exports.tearDown(test, testCommon)
}
