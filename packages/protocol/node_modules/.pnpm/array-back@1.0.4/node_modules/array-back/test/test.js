'use strict'
var TestRunner = require('test-runner')
var arrayify = require('../')
var a = require('core-assert')

var runner = new TestRunner()

runner.test('arrayify()', function () {
  a.deepStrictEqual(arrayify(undefined), [])
  a.deepStrictEqual(arrayify(null), [ null ])
  a.deepStrictEqual(arrayify(0), [ 0 ])
  a.deepStrictEqual(arrayify([ 1, 2 ]), [ 1, 2 ])

  function func () {
    a.deepStrictEqual(arrayify(arguments), [ 1, 2, 3 ])
  }
  func(1, 2, 3)
})
