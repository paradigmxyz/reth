'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('getOpt short notation: two flags, one option', function () {
  const optionDefinitions = [
    { name: 'flagA', alias: 'a' },
    { name: 'flagB', alias: 'b' },
    { name: 'three', alias: 'c' }
  ]

  const argv = [ '-abc', 'yeah' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    flagA: null,
    flagB: null,
    three: 'yeah'
  })
})

runner.test('option=value notation: two plus a regular notation', function () {
  const optionDefinitions = [
    { name: 'one' },
    { name: 'two' },
    { name: 'three' }
  ]

  const argv = [ '--one=1', '--two', '2', '--three=3' ]
  const result = commandLineArgs(optionDefinitions, { argv })
  a.strictEqual(result.one, '1')
  a.strictEqual(result.two, '2')
  a.strictEqual(result.three, '3')
})

runner.test('option=value notation: value contains "="', function () {
  const optionDefinitions = [
    { name: 'url' },
    { name: 'two' },
    { name: 'three' }
  ]

  let result = commandLineArgs(optionDefinitions, { argv: [ '--url=my-url?q=123', '--two', '2', '--three=3' ] })
  a.strictEqual(result.url, 'my-url?q=123')
  a.strictEqual(result.two, '2')
  a.strictEqual(result.three, '3')

  result = commandLineArgs(optionDefinitions, { argv: [ '--url=my-url?q=123=1' ] })
  a.strictEqual(result.url, 'my-url?q=123=1')

  result = commandLineArgs({ name: 'my-url' }, { argv: [ '--my-url=my-url?q=123=1' ] })
  a.strictEqual(result['my-url'], 'my-url?q=123=1')
})
