'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('type-boolean: different values', function () {
  const definitions = [
    { name: 'one', type: Boolean }
  ]

  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'true' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'false' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'sfsgf' ] }),
    { one: true }
  )
})

const origBoolean = Boolean

/* test in contexts which override the standard global Boolean constructor */
runner.test('type-boolean: global Boolean overridden', function () {
  function Boolean () {
    return origBoolean.apply(origBoolean, arguments)
  }

  const definitions = [
    { name: 'one', type: Boolean }
  ]

  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'true' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'false' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'sfsgf' ] }),
    { one: true }
  )
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one' ] }),
    { one: true }
  )
})

runner.test('type-boolean-multiple: 1', function () {
  const definitions = [
    { name: 'array', type: Boolean, multiple: true }
  ]
  const argv = [ '--array', '--array', '--array' ]
  const result = commandLineArgs(definitions, { argv })
  a.deepStrictEqual(result, {
    array: [ true, true, true ]
  })
})
