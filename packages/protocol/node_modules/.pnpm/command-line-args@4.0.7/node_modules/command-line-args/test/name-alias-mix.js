'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

const optionDefinitions = [
  { name: 'one', alias: 'o' },
  { name: 'two', alias: 't' },
  { name: 'three', alias: 'h' },
  { name: 'four', alias: 'f' }
]

runner.test('name-alias-mix: one of each', function () {
  const argv = [ '--one', '-t', '--three' ]
  const result = commandLineArgs(optionDefinitions, { argv })
  a.strictEqual(result.one, null)
  a.strictEqual(result.two, null)
  a.strictEqual(result.three, null)
  a.strictEqual(result.four, undefined)
})
