'use strict'
var TestRunner = require('test-runner')
var testValue = require('../')
var a = require('core-assert')

function TestClass () {
  this.one = 1
}

var testClass = new TestClass()
var runner = new TestRunner()

var fixture = {
  result: 'clive',
  hater: true,
  colour: 'red-ish',
  deep: {
    name: 'Zhana',
    favourite: {
      colour: [ 'white', 'red' ]
    },
    arr: [ 1, 2, 3 ]
  },
  nullVal: null,
  boolTrue: true,
  number: 5,
  testClass: testClass,
  arr: [ 1, 2, 3 ],
  arrObjects: [
    { number: 1 },
    { number: 2 }
  ]
}

runner.test('testValue(obj, { property: primative })', function () {
  a.strictEqual(testValue(fixture, { result: 'clive' }), true)
  a.strictEqual(testValue(fixture, { hater: true }), true)
  a.strictEqual(testValue(fixture, { result: 'clive', hater: true }), true)
  a.strictEqual(testValue(fixture, { ibe: true }), false)
})

runner.test('testValue(obj, { !property: primative })', function () {
  a.strictEqual(testValue(fixture, { '!result': 'clive' }), false)
  a.strictEqual(testValue(fixture, { '!result': 'ian' }), true)
  a.strictEqual(testValue(fixture, { '!result': 'ian', '!hater': false }), true)
})

runner.test('testValue(obj, { property: primative[] })', function () {
  a.strictEqual(testValue(fixture, { arr: [ 1, 2, 3 ] }), true)
  a.strictEqual(testValue(fixture, { arr: [ /1/ ] }), true)
  a.strictEqual(testValue(fixture, { arr: [ /4/ ] }), false)
  a.strictEqual(testValue(fixture, { colour: [ 1, 2, 3 ] }), false, 'querying a string with array')
  a.strictEqual(testValue(fixture, { undefinedProperty: [ 1, 2, 3 ] }), false, 'querying undefined property')
  a.strictEqual(testValue(fixture, { undefinedProperty: [ undefined ] }), true)
  a.strictEqual(testValue(fixture, { undefinedProperty: [ null ] }), false)
})

runner.test('testValue(obj, { property: { property: primative[] } })', function () {
  a.strictEqual(testValue(fixture, { deep: { arr: [ 1, 2 ] } }), true)
  a.strictEqual(testValue(fixture, { deep: { arr: [ 3, 4 ] } }), true)
  a.strictEqual(testValue(fixture, { deep: { favourite: { colour: [ 'white', 'red' ] } } }), true)
})

runner.test('testValue(obj, { property: undefined, property: regex })', function () {
  a.strictEqual(testValue(fixture.deep, { undefinedProperty: undefined, name: /.+/ }), true)
})

runner.test('testValue(obj, { property: /regex/ })', function () {
  a.strictEqual(testValue(fixture, { colour: /red/ }), true)
  a.strictEqual(testValue(fixture, { colour: /black/ }), false)
  a.strictEqual(testValue(fixture, { colour: /RED/i }), true)
  a.strictEqual(testValue(fixture, { colour: /.+/ }), true)
  a.strictEqual(testValue(fixture, { undefinedProperty: /.+/ }), false, 'testing undefined val')
  a.strictEqual(testValue(fixture, { deep: /.+/ }), false, 'testing an object val')
  a.strictEqual(testValue(fixture, { nullVal: /.+/ }), false, 'testing a null val')
  a.strictEqual(testValue(fixture, { boolTrue: /true/ }), true, 'testing a boolean val')
  a.strictEqual(testValue(fixture, { boolTrue: /addf/ }), false, 'testing a boolean val')
})

runner.test('testValue(obj, { !property: /regex/ })', function () {
  a.strictEqual(testValue(fixture, { '!colour': /red/ }), false)
  a.strictEqual(testValue(fixture, { '!colour': /black/ }), true)
  a.strictEqual(testValue(fixture, { '!colour': /blue/ }), true)
})

runner.test('testValue(obj, { property: function })', function () {
  a.strictEqual(testValue(fixture, { number: function (n) { return n < 4 } }), false, '< 4')
  a.strictEqual(testValue(fixture, { number: function (n) { return n < 10 } }), true, '< 10')
})

runner.test('testValue(obj, { !property: function })', function () {
  a.strictEqual(testValue(fixture, { '!number': function (n) { return n < 10 } }), false, '< 10')
})

runner.test('testValue(obj, { property: object })', function () {
  a.strictEqual(testValue(fixture, { testClass: { one: 1 } }), true, 'querying a plain object')
  a.strictEqual(testValue(fixture, { testClass: testClass }), true, 'querying an object instance')
})

runner.test('testValue(obj, { +property: primitive })', function () {
  a.strictEqual(testValue(fixture, { arr: 1 }), false)
  a.strictEqual(testValue(fixture, { '+arr': 1 }), true)
})

runner.test('testValue(obj, { property. { +property: query } })', function () {
  a.strictEqual(testValue(fixture, { deep: { favourite: { '+colour': 'red' } } }), true)
  a.strictEqual(testValue(fixture, { deep: { favourite: { '+colour': /red/ } } }), true)
  a.strictEqual(testValue(fixture, { deep: { favourite: { '+colour': function (c) {
    return c === 'red'
  } } } }), true)
  a.strictEqual(testValue(fixture, { deep: { favourite: { '+colour': /green/ } } }), false)
})

runner.test('testValue(obj, { +property: query })', function () {
  a.strictEqual(testValue(fixture, { arrObjects: { number: 1 } }), false)
  a.strictEqual(testValue(fixture, { '+arrObjects': { number: 1 } }), true)
})

runner.test('object deep exists, summary', function () {
  var query = {
    one: {
      one: {
        three: 'three',
        '!four': 'four'
      },
      two: {
        one: {
          one: 'one'
        },
        '!two': undefined,
        '!three': [ { '!one': { '!one': '110' } } ]
      }
    }
  }

  var obj1 = {
    one: {
      one: {
        one: 'one',
        two: 'two',
        three: 'three'
      },
      two: {
        one: {
          one: 'one'
        },
        two: 2
      }
    }
  }

  var obj2 = {
    one: {
      one: {
        one: 'one',
        two: 'two'
      },
      two: {
        one: {
          one: 'one'
        },
        two: 2
      }
    }
  }

  var obj3 = {
    one: {
      one: {
        one: 'one',
        two: 'two',
        three: 'three'
      },
      two: {
        one: {
          one: 'one'
        },
        two: 2,
        three: [
          { one: { one: '100' } },
          { one: { one: '110' } }
        ]
      }
    }
  }

  var obj4 = {
    one: {
      one: {
        one: 'one',
        two: 'two',
        three: 'three'
      },
      two: {
        one: {
          one: 'one'
        },
        two: 2,
        three: [
          { one: { one: '100' } }
        ]
      }
    }
  }

  a.strictEqual(testValue(obj1, query), true, 'true obj1')
  a.strictEqual(testValue(obj2, query), false, 'false obj2')
  a.strictEqual(testValue(obj3, query), false, 'false in obj3')
  a.strictEqual(testValue(obj4, query), true, 'true in obj4')
})

runner.test('testValue.where({ property: primative })', function () {
  var arr = [
    { num: 1 }, { num: 2 }, { num: 3 }
  ]
  a.strictEqual(arr.some(testValue.where({ num: 2 })), true)
  a.strictEqual(arr.some(testValue.where({ num: 4 })), false)
  a.deepEqual(arr.filter(testValue.where({ num: 2 })), [ { num: 2 } ])
  a.deepEqual(arr.filter(testValue.where({ num: 4 })), [])
})

runner.test('testValue(val, object, { strict: true })', function () {
  var obj1 = { one: 1 }
  var query1 = { one: 1 }
  var query2 = { two: 2 }
  a.strictEqual(testValue(obj1, query1), true)
  a.strictEqual(testValue(obj1, query1, { strict: true }), false)
  a.strictEqual(testValue(obj1, query2), false)
  a.strictEqual(testValue(obj1, query2, { strict: true }), false)
  a.strictEqual(testValue(obj1, [ query1 ]), true)
  a.strictEqual(testValue(obj1, [ query1 ], { strict: true }), false)
  a.strictEqual(testValue(obj1, [ query2 ]), false)
  a.strictEqual(testValue(obj1, [ query2 ], { strict: true }), false)
  a.strictEqual(testValue(obj1, [ query1, query2 ]), true)
  a.strictEqual(testValue(obj1, [ query1, query2 ], { strict: true }), false)
  a.strictEqual(testValue(obj1, [ query1, query2, obj1 ], { strict: true }), true)

  a.strictEqual(testValue(Object.getPrototypeOf([ 1, 2 ]), Array.prototype), true)
  a.strictEqual(testValue(Object.getPrototypeOf([ 1, 2 ]), [ Array.prototype ]), true)
  a.strictEqual(testValue(Object.getPrototypeOf([ 1, 2 ]), Array.prototype, { strict: true }), true)
  a.strictEqual(testValue(Object.getPrototypeOf([ 1, 2 ]), [ Array.prototype ], { strict: true }), true)

  function one () {}
  a.strictEqual(testValue(one, Function.prototype), false)
  a.strictEqual(testValue(Object.getPrototypeOf(one), Function.prototype), true)
  a.strictEqual(testValue(Object.getPrototypeOf(one), [ Function.prototype ]), true)
  a.strictEqual(testValue(Object.getPrototypeOf(one), Function.prototype, { strict: true }), true)
  a.strictEqual(testValue(Object.getPrototypeOf(one), [ Function.prototype ], { strict: true }), true)

  a.strictEqual(testValue({}, Object.prototype), false)
  a.strictEqual(testValue(Object.getPrototypeOf({}), Object.prototype), true)
  a.strictEqual(testValue(Object.getPrototypeOf({}), [ Object.prototype ]), true)
  a.strictEqual(testValue(Object.getPrototypeOf({}), Object.prototype, { strict: true }), true)
  a.strictEqual(testValue(Object.getPrototypeOf({}), [ Object.prototype ], { strict: true }), true)
})
