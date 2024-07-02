'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

const optionDefinitions = [
  { name: 'verbose', alias: 'v' },
  { name: 'colour', alias: 'c' },
  { name: 'number', alias: 'n' },
  { name: 'dry-run', alias: 'd' }
]

runner.test('alias: one boolean', function () {
  const argv = [ '-v' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    verbose: null
  })
})

runner.test('alias: one --this-type boolean', function () {
  const argv = [ '-d' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    'dry-run': null
  })
})

runner.test('alias: one boolean, one string', function () {
  const argv = [ '-v', '-c' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    verbose: null,
    colour: null
  })
})
