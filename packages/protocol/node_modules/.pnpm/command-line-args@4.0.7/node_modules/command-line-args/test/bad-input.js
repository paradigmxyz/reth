'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('bad-input: handles missing option value', function () {
  const definitions = [
    { name: 'colour', type: String },
    { name: 'files' }
  ]
  a.deepStrictEqual(commandLineArgs(definitions, { argv: [ '--colour' ] }), {
    colour: null
  })
  a.deepStrictEqual(commandLineArgs(definitions, { argv: [ '--colour', '--files', 'yeah' ] }), {
    colour: null,
    files: 'yeah'
  })
})

runner.test('bad-input: handles arrays with relative paths', function () {
  const definitions = [
    { name: 'colours', type: String, multiple: true }
  ]
  const argv = [ '--colours', '../what', '../ever' ]
  a.deepStrictEqual(commandLineArgs(definitions, { argv }), {
    colours: [ '../what', '../ever' ]
  })
})

runner.test('bad-input: empty string', function () {
  const definitions = [
    { name: 'one', type: String },
    { name: 'two', type: Number },
    { name: 'three', type: Number, multiple: true },
    { name: 'four', type: String },
    { name: 'five', type: Boolean }
  ]
  const argv = [ '--one', '', '', '--two', '0', '--three=', '', '--four=', '--five=' ]
  a.deepStrictEqual(commandLineArgs(definitions, { argv }), {
    one: '',
    two: 0,
    three: [ 0, 0 ],
    four: '',
    five: true
  })
})

runner.test('bad-input: non-strings in argv', function () {
  const definitions = [
    { name: 'one', type: Number }
  ]
  const argv = [ '--one', 1 ]
  const result = commandLineArgs(definitions, { argv })
  a.deepStrictEqual(result, { one: 1 })
})
