'use strict'

var test = require('tape')
var sinon = require('sinon')
var inherits = require('util').inherits
var testCommon = require('./testCommon')
var AbstractLevelDOWN = require('./').AbstractLevelDOWN
var AbstractIterator = require('./').AbstractIterator
var AbstractChainedBatch = require('./').AbstractChainedBatch

function factory (location) {
  return new AbstractLevelDOWN(location)
}

/**
 * Compatibility with basic LevelDOWN API
 */

require('./abstract/leveldown-test').args(factory, test)

require('./abstract/open-test').args(factory, test, testCommon)

require('./abstract/del-test').setUp(factory, test, testCommon)
require('./abstract/del-test').args(test)

require('./abstract/get-test').setUp(factory, test, testCommon)
require('./abstract/get-test').args(test)

require('./abstract/put-test').setUp(factory, test, testCommon)
require('./abstract/put-test').args(test)

require('./abstract/put-get-del-test').setUp(factory, test, testCommon)
require('./abstract/put-get-del-test').errorKeys(test)
// require('./abstract/put-get-del-test').nonErrorKeys(test, testCommon)
require('./abstract/put-get-del-test').errorValues()
require('./abstract/put-get-del-test').tearDown(test, testCommon)

require('./abstract/batch-test').setUp(factory, test, testCommon)
require('./abstract/batch-test').args(test)

require('./abstract/chained-batch-test').setUp(factory, test, testCommon)
require('./abstract/chained-batch-test').args(test)

require('./abstract/close-test').close(factory, test, testCommon)

require('./abstract/iterator-test').setUp(factory, test, testCommon)
require('./abstract/iterator-test').args(test)
require('./abstract/iterator-test').sequence(test)

function implement (ctor, methods) {
  function Test () {
    ctor.apply(this, arguments)
  }

  inherits(Test, ctor)

  for (var k in methods) {
    Test.prototype[k] = methods[k]
  }

  return Test
}

/**
 * Extensibility
 */

test('test core extensibility', function (t) {
  var Test = implement(AbstractLevelDOWN)
  var test = new Test('foobar')
  t.equal(test.location, 'foobar', 'location set on instance')
  t.end()
})

test('test key/value serialization', function (t) {
  var Test = implement(AbstractLevelDOWN)
  var buffer = Buffer.alloc(0)
  var test = new Test('foobar')

  t.equal(test._serializeKey(1), '1', '_serializeKey converts to string')
  t.ok(test._serializeKey(buffer) === buffer, '_serializeKey returns Buffer as is')

  t.equal(test._serializeValue(null), '', '_serializeValue converts null to empty string')
  t.equal(test._serializeValue(undefined), '', '_serializeValue converts undefined to empty string')

  var browser = !!process.browser
  process.browser = false

  t.equal(test._serializeValue(1), '1', '_serializeValue converts to string')
  t.ok(test._serializeValue(buffer) === buffer, '_serializeValue returns Buffer as is')

  process.browser = true
  t.equal(test._serializeValue(1), 1, '_serializeValue returns value as is when process.browser')

  process.browser = browser

  t.end()
})

test('test open() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { createIfMissing: true, errorIfExists: false }
  var Test = implement(AbstractLevelDOWN, { _open: spy })
  var test = new Test('foobar')

  test.open(expectedCb)

  t.equal(spy.callCount, 1, 'got _open() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _open() was correct')
  t.equal(spy.getCall(0).args.length, 2, 'got two arguments')
  t.deepEqual(spy.getCall(0).args[0], expectedOptions, 'got default options argument')

  test.open({ options: 1 }, expectedCb)

  expectedOptions.options = 1

  t.equal(spy.callCount, 2, 'got _open() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _open() was correct')
  t.equal(spy.getCall(1).args.length, 2, 'got two arguments')
  t.deepEqual(spy.getCall(1).args[0], expectedOptions, 'got expected options argument')
  t.end()
})

test('test close() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var Test = implement(AbstractLevelDOWN, { _close: spy })
  var test = new Test('foobar')

  test.close(expectedCb)

  t.equal(spy.callCount, 1, 'got _close() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _close() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one arguments')
  t.end()
})

test('test get() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { asBuffer: true }
  var expectedKey = 'a key'
  var Test = implement(AbstractLevelDOWN, { _get: spy })
  var test = new Test('foobar')

  test.get(expectedKey, expectedCb)

  t.equal(spy.callCount, 1, 'got _get() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _get() was correct')
  t.equal(spy.getCall(0).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(0).args[0], expectedKey, 'got expected key argument')
  t.deepEqual(spy.getCall(0).args[1], expectedOptions, 'got default options argument')
  t.equal(spy.getCall(0).args[2], expectedCb, 'got expected cb argument')

  test.get(expectedKey, { options: 1 }, expectedCb)

  expectedOptions.options = 1

  t.equal(spy.callCount, 2, 'got _get() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _get() was correct')
  t.equal(spy.getCall(1).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(1).args[0], expectedKey, 'got expected key argument')
  t.deepEqual(spy.getCall(1).args[1], expectedOptions, 'got expected options argument')
  t.equal(spy.getCall(1).args[2], expectedCb, 'got expected cb argument')
  t.end()
})

test('test del() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { options: 1 }
  var expectedKey = 'a key'
  var Test = implement(AbstractLevelDOWN, { _del: spy })
  var test = new Test('foobar')

  test.del(expectedKey, expectedCb)

  t.equal(spy.callCount, 1, 'got _del() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _del() was correct')
  t.equal(spy.getCall(0).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(0).args[0], expectedKey, 'got expected key argument')
  t.deepEqual(spy.getCall(0).args[1], {}, 'got blank options argument')
  t.equal(spy.getCall(0).args[2], expectedCb, 'got expected cb argument')

  test.del(expectedKey, expectedOptions, expectedCb)

  t.equal(spy.callCount, 2, 'got _del() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _del() was correct')
  t.equal(spy.getCall(1).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(1).args[0], expectedKey, 'got expected key argument')
  t.deepEqual(spy.getCall(1).args[1], expectedOptions, 'got expected options argument')
  t.equal(spy.getCall(1).args[2], expectedCb, 'got expected cb argument')
  t.end()
})

test('test put() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { options: 1 }
  var expectedKey = 'a key'
  var expectedValue = 'a value'
  var Test = implement(AbstractLevelDOWN, { _put: spy })
  var test = new Test('foobar')

  test.put(expectedKey, expectedValue, expectedCb)

  t.equal(spy.callCount, 1, 'got _put() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _put() was correct')
  t.equal(spy.getCall(0).args.length, 4, 'got four arguments')
  t.equal(spy.getCall(0).args[0], expectedKey, 'got expected key argument')
  t.equal(spy.getCall(0).args[1], expectedValue, 'got expected value argument')
  t.deepEqual(spy.getCall(0).args[2], {}, 'got blank options argument')
  t.equal(spy.getCall(0).args[3], expectedCb, 'got expected cb argument')

  test.put(expectedKey, expectedValue, expectedOptions, expectedCb)

  t.equal(spy.callCount, 2, 'got _put() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _put() was correct')
  t.equal(spy.getCall(1).args.length, 4, 'got four arguments')
  t.equal(spy.getCall(1).args[0], expectedKey, 'got expected key argument')
  t.equal(spy.getCall(1).args[1], expectedValue, 'got expected value argument')
  t.deepEqual(spy.getCall(1).args[2], expectedOptions, 'got blank options argument')
  t.equal(spy.getCall(1).args[3], expectedCb, 'got expected cb argument')
  t.end()
})

test('test batch() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { options: 1 }
  var expectedArray = [
    { type: 'put', key: '1', value: '1' },
    { type: 'del', key: '2' }
  ]
  var Test = implement(AbstractLevelDOWN, { _batch: spy })
  var test = new Test('foobar')

  test.batch(expectedArray, expectedCb)

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _batch() was correct')
  t.equal(spy.getCall(0).args.length, 3, 'got three arguments')
  t.deepEqual(spy.getCall(0).args[0], expectedArray, 'got expected array argument')
  t.deepEqual(spy.getCall(0).args[1], {}, 'got expected options argument')
  t.equal(spy.getCall(0).args[2], expectedCb, 'got expected callback argument')

  test.batch(expectedArray, expectedOptions, expectedCb)

  t.equal(spy.callCount, 2, 'got _batch() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _batch() was correct')
  t.equal(spy.getCall(1).args.length, 3, 'got three arguments')
  t.deepEqual(spy.getCall(1).args[0], expectedArray, 'got expected array argument')
  t.deepEqual(spy.getCall(1).args[1], expectedOptions, 'got expected options argument')
  t.equal(spy.getCall(1).args[2], expectedCb, 'got expected callback argument')

  test.batch(expectedArray, null, expectedCb)

  t.equal(spy.callCount, 3, 'got _batch() call')
  t.equal(spy.getCall(2).thisValue, test, '`this` on _batch() was correct')
  t.equal(spy.getCall(2).args.length, 3, 'got three arguments')
  t.deepEqual(spy.getCall(2).args[0], expectedArray, 'got expected array argument')
  t.ok(spy.getCall(2).args[1], 'options should not be null')
  t.equal(spy.getCall(2).args[2], expectedCb, 'got expected callback argument')
  t.end()
})

test('test chained batch() (array) extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var expectedOptions = { options: 1 }
  var Test = implement(AbstractLevelDOWN, { _batch: spy })
  var test = new Test('foobar')

  test.batch().put('foo', 'bar').del('bang').write(expectedCb)

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _batch() was correct')
  t.equal(spy.getCall(0).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(0).args[0].length, 2, 'got expected array argument')
  t.deepEqual(spy.getCall(0).args[0][0], { type: 'put', key: 'foo', value: 'bar' }, 'got expected array argument[0]')
  t.deepEqual(spy.getCall(0).args[0][1], { type: 'del', key: 'bang' }, 'got expected array argument[1]')
  t.deepEqual(spy.getCall(0).args[1], {}, 'got expected options argument')
  t.equal(spy.getCall(0).args[2], expectedCb, 'got expected callback argument')

  test.batch().put('foo', 'bar').del('bang').write(expectedOptions, expectedCb)

  t.equal(spy.callCount, 2, 'got _batch() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _batch() was correct')
  t.equal(spy.getCall(1).args.length, 3, 'got three arguments')
  t.equal(spy.getCall(1).args[0].length, 2, 'got expected array argument')
  t.deepEqual(spy.getCall(1).args[0][0], { type: 'put', key: 'foo', value: 'bar' }, 'got expected array argument[0]')
  t.deepEqual(spy.getCall(1).args[0][1], { type: 'del', key: 'bang' }, 'got expected array argument[1]')
  t.deepEqual(spy.getCall(1).args[1], expectedOptions, 'got expected options argument')
  t.equal(spy.getCall(1).args[2], expectedCb, 'got expected callback argument')

  t.end()
})

test('test chained batch() (custom _chainedBatch) extensibility', function (t) {
  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, { _chainedBatch: spy })
  var test = new Test('foobar')

  test.batch()

  t.equal(spy.callCount, 1, 'got _chainedBatch() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _chainedBatch() was correct')

  test.batch()

  t.equal(spy.callCount, 2, 'got _chainedBatch() call')
  t.equal(spy.getCall(1).thisValue, test, '`this` on _chainedBatch() was correct')

  t.end()
})

test('test AbstractChainedBatch extensibility', function (t) {
  var Test = implement(AbstractChainedBatch)
  var test = new Test('foobar')
  t.equal(test._db, 'foobar', 'db set on instance')
  t.end()
})

test('test write() extensibility', function (t) {
  var spy = sinon.spy()
  var spycb = sinon.spy()
  var Test = implement(AbstractChainedBatch, { _write: spy })
  var test = new Test('foobar')

  test.write(spycb)

  t.equal(spy.callCount, 1, 'got _write() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _write() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one argument')
  // awkward here cause of nextTick & an internal wrapped cb
  t.equal(typeof spy.getCall(0).args[0], 'function', 'got a callback function')
  t.equal(spycb.callCount, 0, 'spycb not called')
  spy.getCall(0).args[0]()
  t.equal(spycb.callCount, 1, 'spycb called, i.e. was our cb argument')
  t.end()
})

test('test put() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedKey = 'key'
  var expectedValue = 'value'
  var Test = implement(AbstractChainedBatch, { _put: spy })
  var test = new Test(factory('foobar'))
  var returnValue = test.put(expectedKey, expectedValue)

  t.equal(spy.callCount, 1, 'got _put call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _put() was correct')
  t.equal(spy.getCall(0).args.length, 2, 'got two arguments')
  t.equal(spy.getCall(0).args[0], expectedKey, 'got expected key argument')
  t.equal(spy.getCall(0).args[1], expectedValue, 'got expected value argument')
  t.equal(returnValue, test, 'get expected return value')
  t.end()
})

test('test del() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedKey = 'key'
  var Test = implement(AbstractChainedBatch, { _del: spy })
  var test = new Test(factory('foobar'))
  var returnValue = test.del(expectedKey)

  t.equal(spy.callCount, 1, 'got _del call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _del() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one argument')
  t.equal(spy.getCall(0).args[0], expectedKey, 'got expected key argument')
  t.equal(returnValue, test, 'get expected return value')
  t.end()
})

test('test clear() extensibility', function (t) {
  var spy = sinon.spy()
  var Test = implement(AbstractChainedBatch, { _clear: spy })
  var test = new Test(factory('foobar'))
  var returnValue = test.clear()

  t.equal(spy.callCount, 1, 'got _clear call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _clear() was correct')
  t.equal(spy.getCall(0).args.length, 0, 'got zero arguments')
  t.equal(returnValue, test, 'get expected return value')
  t.end()
})

test('test iterator() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedOptions = {
    options: 1,
    reverse: false,
    keys: true,
    values: true,
    limit: -1,
    keyAsBuffer: true,
    valueAsBuffer: true
  }
  var Test = implement(AbstractLevelDOWN, { _iterator: spy })
  var test = new Test('foobar')

  test.iterator({ options: 1 })

  t.equal(spy.callCount, 1, 'got _close() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _close() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one arguments')
  t.deepEqual(spy.getCall(0).args[0], expectedOptions, 'got expected options argument')
  t.end()
})

test('test AbstractIterator extensibility', function (t) {
  var Test = implement(AbstractIterator)
  var test = new Test('foobar')
  t.equal(test.db, 'foobar', 'db set on instance')
  t.end()
})

test('test next() extensibility', function (t) {
  var spy = sinon.spy()
  var spycb = sinon.spy()
  var Test = implement(AbstractIterator, { _next: spy })
  var test = new Test('foobar')

  test.next(spycb)

  t.equal(spy.callCount, 1, 'got _next() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _next() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one arguments')
  // awkward here cause of nextTick & an internal wrapped cb
  t.equal(typeof spy.getCall(0).args[0], 'function', 'got a callback function')
  t.equal(spycb.callCount, 0, 'spycb not called')
  spy.getCall(0).args[0]()
  t.equal(spycb.callCount, 1, 'spycb called, i.e. was our cb argument')
  t.end()
})

test('test end() extensibility', function (t) {
  var spy = sinon.spy()
  var expectedCb = function () {}
  var Test = implement(AbstractIterator, { _end: spy })
  var test = new Test('foobar')

  test.end(expectedCb)

  t.equal(spy.callCount, 1, 'got _end() call')
  t.equal(spy.getCall(0).thisValue, test, '`this` on _end() was correct')
  t.equal(spy.getCall(0).args.length, 1, 'got one arguments')
  t.equal(spy.getCall(0).args[0], expectedCb, 'got expected cb argument')
  t.end()
})

test('test serialization extensibility (put)', function (t) {
  t.plan(5)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _put: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },

    _serializeValue: function (value) {
      t.equal(value, 'nope')
      return 'bar'
    }
  })

  var test = new Test('foobar')
  test.put('no', 'nope', function () {})

  t.equal(spy.callCount, 1, 'got _put() call')
  t.equal(spy.getCall(0).args[0], 'foo', 'got expected key argument')
  t.equal(spy.getCall(0).args[1], 'bar', 'got expected value argument')
})

test('test serialization extensibility (del)', function (t) {
  t.plan(3)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _del: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.fail('should not be called')
    }
  })

  var test = new Test('foobar')
  test.del('no', function () {})

  t.equal(spy.callCount, 1, 'got _del() call')
  t.equal(spy.getCall(0).args[0], 'foo', 'got expected key argument')

  t.end()
})

test('test serialization extensibility (batch array put)', function (t) {
  t.plan(5)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _batch: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.equal(value, 'nope')
      return 'bar'
    }
  })

  var test = new Test('foobar')
  test.batch([ { type: 'put', key: 'no', value: 'nope' } ], function () {})

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).args[0][0].key, 'foo', 'got expected key')
  t.equal(spy.getCall(0).args[0][0].value, 'bar', 'got expected value')
})

test('test serialization extensibility (batch chain put)', function (t) {
  t.plan(5)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _batch: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.equal(value, 'nope')
      return 'bar'
    }
  })

  var test = new Test('foobar')
  test.batch().put('no', 'nope').write(function () {})

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).args[0][0].key, 'foo', 'got expected key')
  t.equal(spy.getCall(0).args[0][0].value, 'bar', 'got expected value')
})

test('test serialization extensibility (batch array del)', function (t) {
  t.plan(3)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _batch: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.fail('should not be called')
    }
  })

  var test = new Test('foobar')
  test.batch([ { type: 'del', key: 'no' } ], function () {})

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).args[0][0].key, 'foo', 'got expected key')
})

test('test serialization extensibility (batch chain del)', function (t) {
  t.plan(3)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _batch: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.fail('should not be called')
    }
  })

  var test = new Test('foobar')
  test.batch().del('no').write(function () {})

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).args[0][0].key, 'foo', 'got expected key')
})

test('test serialization extensibility (batch array is not mutated)', function (t) {
  t.plan(7)

  var spy = sinon.spy()
  var Test = implement(AbstractLevelDOWN, {
    _batch: spy,
    _serializeKey: function (key) {
      t.equal(key, 'no')
      return 'foo'
    },
    _serializeValue: function (value) {
      t.equal(value, 'nope')
      return 'bar'
    }
  })

  var test = new Test('foobar')
  var op = { type: 'put', key: 'no', value: 'nope' }

  test.batch([op], function () {})

  t.equal(spy.callCount, 1, 'got _batch() call')
  t.equal(spy.getCall(0).args[0][0].key, 'foo', 'got expected key')
  t.equal(spy.getCall(0).args[0][0].value, 'bar', 'got expected value')

  t.equal(op.key, 'no', 'did not mutate input key')
  t.equal(op.value, 'nope', 'did not mutate input value')
})

test('.status', function (t) {
  t.test('empty prototype', function (t) {
    var Test = implement(AbstractLevelDOWN)
    var test = new Test('foobar')

    t.equal(test.status, 'new')

    test.open(function (err) {
      t.error(err)
      t.equal(test.status, 'open')

      test.close(function (err) {
        t.error(err)
        t.equal(test.status, 'closed')
        t.end()
      })
    })
  })

  t.test('open error', function (t) {
    var Test = implement(AbstractLevelDOWN, {
      _open: function (options, cb) {
        cb(new Error())
      }
    })

    var test = new Test('foobar')

    test.open(function (err) {
      t.ok(err)
      t.equal(test.status, 'new')
      t.end()
    })
  })

  t.test('close error', function (t) {
    var Test = implement(AbstractLevelDOWN, {
      _close: function (cb) {
        cb(new Error())
      }
    })

    var test = new Test('foobar')
    test.open(function () {
      test.close(function (err) {
        t.ok(err)
        t.equal(test.status, 'open')
        t.end()
      })
    })
  })

  t.test('open', function (t) {
    var Test = implement(AbstractLevelDOWN, {
      _open: function (options, cb) {
        process.nextTick(cb)
      }
    })

    var test = new Test('foobar')
    test.open(function (err) {
      t.error(err)
      t.equal(test.status, 'open')
      t.end()
    })
    t.equal(test.status, 'opening')
  })

  t.test('close', function (t) {
    var Test = implement(AbstractLevelDOWN, {
      _close: function (cb) {
        process.nextTick(cb)
      }
    })

    var test = new Test('foobar')
    test.open(function (err) {
      t.error(err)
      test.close(function (err) {
        t.error(err)
        t.equal(test.status, 'closed')
        t.end()
      })
      t.equal(test.status, 'closing')
    })
  })
})

test('_setupIteratorOptions', function (t) {
  var keys = 'start end gt gte lt lte'.split(' ')
  var db = new AbstractLevelDOWN('foolocation')

  function setupOptions (constrFn) {
    var options = {}
    keys.forEach(function (key) {
      options[key] = constrFn()
    })
    return options
  }

  function verifyUndefinedOptions (t, options) {
    keys.forEach(function (key) {
      t.notOk(key in options, 'property should be deleted')
    })
    t.end()
  }

  t.test('default options', function (t) {
    t.same(db._setupIteratorOptions(), {
      reverse: false,
      keys: true,
      values: true,
      limit: -1,
      keyAsBuffer: true,
      valueAsBuffer: true
    }, 'correct defaults')
    t.end()
  })

  t.test('set options', function (t) {
    t.same(db._setupIteratorOptions({
      reverse: false,
      keys: false,
      values: false,
      limit: 20,
      keyAsBuffer: false,
      valueAsBuffer: false
    }), {
      reverse: false,
      keys: false,
      values: false,
      limit: 20,
      keyAsBuffer: false,
      valueAsBuffer: false
    }, 'options set correctly')
    t.end()
  })

  t.test('deletes empty buffers', function (t) {
    var options = setupOptions(function () { return Buffer.from('') })
    keys.forEach(function (key) {
      t.is(Buffer.isBuffer(options[key]), true, 'should be buffer')
      t.is(options[key].length, 0, 'should be empty')
    })
    verifyUndefinedOptions(t, db._setupIteratorOptions(options))
  })

  t.test('deletes empty strings', function (t) {
    var options = setupOptions(function () { return '' })
    keys.forEach(function (key) {
      t.is(typeof options[key], 'string', 'should be string')
      t.is(options[key].length, 0, 'should be empty')
    })
    verifyUndefinedOptions(t, db._setupIteratorOptions(options))
  })

  t.test('deletes null options', function (t) {
    var options = setupOptions(function () { return null })
    keys.forEach(function (key) {
      t.same(options[key], null, 'should be null')
    })
    verifyUndefinedOptions(t, db._setupIteratorOptions(options))
  })
})
