'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('detect process.argv: should automatically remove first two argv items', function () {
  process.argv = [ 'node', 'filename', '--one', 'eins' ]
  a.deepStrictEqual(commandLineArgs({ name: 'one' }, { argv: process.argv }), {
    one: 'eins'
  })
})

runner.test('process.argv is left untouched', function () {
  process.argv = [ 'node', 'filename', '--one', 'eins' ]
  a.deepStrictEqual(commandLineArgs({ name: 'one' }), {
    one: 'eins'
  })
  a.deepStrictEqual(process.argv, [ 'node', 'filename', '--one', 'eins' ])
})
