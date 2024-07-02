'use strict'
const TestRunner = require('test-runner')
const findReplace = require('../')
const a = require('assert')

const runner = new TestRunner()

function fixture () {
  return [ 1, 2, 3, 4, 2 ]
}
function argv () {
  return [ '--one', '1', '-abc', 'three' ]
}

runner.test('find primitive, replace with primitive', function (t) {
  a.deepStrictEqual(
    findReplace(fixture(), 2, 'two'),
    [ 1, 'two', 3, 4, 'two' ]
  )
})

runner.test('find primitive, replace with array', function (t) {
  a.deepStrictEqual(
    findReplace(fixture(), 2, [ 'two', 'zwei' ]),
    [ 1, [ 'two', 'zwei' ], 3, 4, [ 'two', 'zwei' ] ]
  )
})

runner.test('find primitive, replace with several primitives', function (t) {
  a.deepStrictEqual(
    findReplace(fixture(), 2, 'two', 'zwei'),
    [ 1, 'two', 'zwei', 3, 4, 'two', 'zwei' ]
  )
})

runner.test('getopt example', function (t) {
  a.deepStrictEqual(
    findReplace(argv(), /^-(\w{2,})$/, function (match) {
      return [ '-a', '-b', '-c' ]
    }),
    [ '--one', '1', '-a', '-b', '-c', 'three' ]
  )
})

runner.test('getopt example 2', function (t) {
  a.deepStrictEqual(
    findReplace(argv(), /^-(\w{2,})$/, 'bread', 'milk'),
    [ '--one', '1', 'bread', 'milk', 'three' ]
  )
})
