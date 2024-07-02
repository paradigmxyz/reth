'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('ambiguous input: value looks like option', function () {
  const optionDefinitions = [
    { name: 'colour', type: String, alias: 'c' }
  ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv: [ '-c', 'red' ] }), {
    colour: 'red'
  })
  a.throws(function () {
    commandLineArgs(optionDefinitions, { argv: [ '--colour', '--red' ] })
  })
  a.doesNotThrow(function () {
    commandLineArgs(optionDefinitions, { argv: [ '--colour=--red' ] })
  })
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv: [ '--colour=--red' ] }), {
    colour: '--red'
  })
})
