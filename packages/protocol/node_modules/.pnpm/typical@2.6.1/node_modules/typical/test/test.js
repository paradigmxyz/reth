'use strict'
var TestRunner = require('test-runner')
var type = require('../')
var detect = require('feature-detect-es6')
var runner = new TestRunner()
var a = require('core-assert')

function evaluates (statement) {
  try {
    eval(statement)
    return true
  } catch (err) {
    return false
  }
}

runner.test('.isNumber(value)', function () {
  a.strictEqual(type.isNumber(0), true)
  a.strictEqual(type.isNumber(1), true)
  a.strictEqual(type.isNumber(1.1), true)
  a.strictEqual(type.isNumber(0xff), true)
  a.strictEqual(type.isNumber(6.2e5), true)
  a.strictEqual(type.isNumber(NaN), false)
  a.strictEqual(type.isNumber(Infinity), false)
})

runner.test('.isPlainObject(value)', function () {
  a.strictEqual(type.isPlainObject({ clive: 'hater' }), true, '{} is true')
  a.strictEqual(type.isPlainObject(new Date()), false, 'new Date() is false')
  a.strictEqual(type.isPlainObject([ 0, 1 ]), false, 'Array is false')
  a.strictEqual(type.isPlainObject(/test/), false, 'RegExp is false')
  a.strictEqual(type.isPlainObject(1), false, '1 is false')
  a.strictEqual(type.isPlainObject('one'), false, "'one' is false")
  a.strictEqual(type.isPlainObject(null), false, 'null is false')
})

runner.test('.isDefined(value)', function () {
  a.strictEqual(type.isDefined({}), true)
  a.strictEqual(type.isDefined({}.one), false)
  a.strictEqual(type.isDefined(0), true)
  a.strictEqual(type.isDefined(null), true)
  a.strictEqual(type.isDefined(undefined), false)
})

runner.test('.isString(value)', function () {
  a.strictEqual(type.isString(0), false)
  a.strictEqual(type.isString('1'), true)
  a.strictEqual(type.isString(1.1), false)
  a.strictEqual(type.isString(NaN), false)
  a.strictEqual(type.isString(Infinity), false)
})

runner.test('.isBoolean(value)', function () {
  a.strictEqual(type.isBoolean(true), true)
  a.strictEqual(type.isBoolean(false), true)
  a.strictEqual(type.isBoolean(0), false)
  a.strictEqual(type.isBoolean('1'), false)
  a.strictEqual(type.isBoolean(1.1), false)
  a.strictEqual(type.isBoolean(NaN), false)
  a.strictEqual(type.isBoolean(Infinity), false)
})

runner.test('.isFunction(value)', function () {
  a.strictEqual(type.isFunction(true), false)
  a.strictEqual(type.isFunction({}), false)
  a.strictEqual(type.isFunction(0), false)
  a.strictEqual(type.isFunction('1'), false)
  a.strictEqual(type.isFunction(1.1), false)
  a.strictEqual(type.isFunction(NaN), false)
  a.strictEqual(type.isFunction(Infinity), false)
  a.strictEqual(type.isFunction(function () {}), true)
  a.strictEqual(type.isFunction(Date), true)
})

runner.test('.isPrimitive(value)', function () {
  a.strictEqual(type.isPrimitive(true), true)
  a.strictEqual(type.isPrimitive({}), false)
  a.strictEqual(type.isPrimitive(0), true)
  a.strictEqual(type.isPrimitive('1'), true)
  a.strictEqual(type.isPrimitive(1.1), true)
  a.strictEqual(type.isPrimitive(NaN), true)
  a.strictEqual(type.isPrimitive(Infinity), true)
  a.strictEqual(type.isPrimitive(function () {}), false)
  a.strictEqual(type.isPrimitive(Date), false)
  a.strictEqual(type.isPrimitive(null), true)
  a.strictEqual(type.isPrimitive(undefined), true)
})

if (detect.symbols() && typeof Symbol() === 'symbol') {
  runner.test('.isPrimitive(value) ES6', function () {
    a.strictEqual(type.isPrimitive(Symbol()), true)
  })
}

runner.test('.isClass(value)', function () {
  a.strictEqual(type.isClass(true), false)
  a.strictEqual(type.isClass({}), false)
  a.strictEqual(type.isClass(0), false)
  a.strictEqual(type.isClass('1'), false)
  a.strictEqual(type.isClass(1.1), false)
  a.strictEqual(type.isClass(NaN), false)
  a.strictEqual(type.isClass(Infinity), false)
  a.strictEqual(type.isClass(function () {}), false)
  a.strictEqual(type.isClass(Date), false)
  a.strictEqual(type.isClass(), false)

  function broken () { }
  broken.toString = function () { throw new Error() }
  a.strictEqual(type.isClass(broken), false)
})

if (detect.class()) {
  runner.test('.isClass(value) ES6', function () {
    var result = eval('type.isClass(class {})')
    a.strictEqual(result, true)
  })
}

if (detect.promises()) {
  runner.test('.isPromise', function () {
    a.strictEqual(type.isPromise(Promise.resolve()), true)
    a.strictEqual(type.isPromise(Promise), false)
    a.strictEqual(type.isPromise(true), false)
    a.strictEqual(type.isPromise({}), false)
    a.strictEqual(type.isPromise(0), false)
    a.strictEqual(type.isPromise('1'), false)
    a.strictEqual(type.isPromise(1.1), false)
    a.strictEqual(type.isPromise(NaN), false)
    a.strictEqual(type.isPromise(Infinity), false)
    a.strictEqual(type.isPromise(function () {}), false)
    a.strictEqual(type.isPromise(Date), false)
    a.strictEqual(type.isPromise(), false)
    a.strictEqual(type.isPromise({ then: function () {} }), true)
  })
}

if (detect.collections()) {
  runner.test('.isIterable', function () {
    a.strictEqual(type.isIterable(Promise.resolve()), false)
    a.strictEqual(type.isIterable(Promise), false)
    a.strictEqual(type.isIterable(true), false)
    a.strictEqual(type.isIterable({}), false)
    a.strictEqual(type.isIterable(0), false)
    a.strictEqual(type.isIterable('1'), true)
    a.strictEqual(type.isIterable(1.1), false)
    a.strictEqual(type.isIterable(NaN), false)
    a.strictEqual(type.isIterable(Infinity), false)
    a.strictEqual(type.isIterable(function () {}), false)
    a.strictEqual(type.isIterable(Date), false)
    a.strictEqual(type.isIterable(), false)
    a.strictEqual(type.isIterable(new Map()), true)
    a.strictEqual(type.isIterable([]), true)
    a.strictEqual(type.isIterable({ then: function () {} }), false)
  })
}
