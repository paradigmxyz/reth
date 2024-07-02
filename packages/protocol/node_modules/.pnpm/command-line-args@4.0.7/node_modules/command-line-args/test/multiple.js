'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('multiple: string unset', function () {
  const argv = []
  const optionDefinitions = [
    { name: 'one', multiple: true }
  ]
  const result = commandLineArgs(optionDefinitions, { argv })
  a.deepStrictEqual(result, { })
})

runner.test('multiple: string unset with defaultValue', function () {
  const argv = []
  const optionDefinitions = [
    { name: 'one', multiple: true, defaultValue: 1 }
  ]
  const result = commandLineArgs(optionDefinitions, { argv })
  a.deepStrictEqual(result, { one: [ 1 ]})
})

runner.test('multiple: boolean unset', function () {
  const argv = []
  const optionDefinitions = [
    { name: 'one', type: Boolean, multiple: true }
  ]
  const result = commandLineArgs(optionDefinitions, { argv })
  a.deepStrictEqual(result, { })
})
