'use strict'
const TestRunner = require('test-runner')
const a = require('assert')
const Output = require('../lib/output')
const Definitions = require('../lib/definitions')

const runner = new TestRunner()

runner.test('output.setFlag(name): initial value', function () {
  let definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number }
  ])
  let output = new Output(definitions)
  a.strictEqual(output.get('one'), undefined)
  output.setFlag('--one')
  a.strictEqual(output.get('one'), null)

  definitions.load([
    { name: 'one', type: Boolean }
  ])
  output = new Output(definitions)
  a.strictEqual(output.get('one'), undefined)
  output.setFlag('--one')
  a.strictEqual(output.get('one'), true)
})

runner.test('output.setOptionValue(name, value)', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number, defaultValue: 1 }
  ])
  const output = new Output(definitions)
  a.strictEqual(output.get('one'), 1)
  output.setOptionValue('--one', '2')
  a.strictEqual(output.get('one'), 2)
})

runner.test('output.setOptionValue(name, value): multiple, defaultValue', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number, multiple: true, defaultValue: [ 1 ] }
  ])
  const output = new Output(definitions)
  a.deepStrictEqual(output.get('one'), [ 1 ])
  output.setOptionValue('--one', '2')
  a.deepStrictEqual(output.get('one'), [ 2 ])
})

runner.test('output.setOptionValue(name, value): multiple 2', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number, multiple: true }
  ])
  const output = new Output(definitions)
  a.deepStrictEqual(output.get('one'), undefined)
  output.setOptionValue('--one', '2')
  a.deepStrictEqual(output.get('one'), [ 2 ])
  output.setOptionValue('--one', '3')
  a.deepStrictEqual(output.get('one'), [ 2, 3 ])
})

runner.test('output.setValue(value): no defaultOption', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number }
  ])
  const output = new Output(definitions)
  a.deepStrictEqual(output.get('one'), undefined)
  output.setValue('2')
  a.deepStrictEqual(output.get('one'), undefined)
  a.deepStrictEqual(output.unknown, [ '2' ])
})

runner.test('output.setValue(value): with defaultOption', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number, defaultOption: true }
  ])
  const output = new Output(definitions)
  a.deepStrictEqual(output.get('one'), undefined)
  output.setValue('2')
  a.deepStrictEqual(output.get('one'), 2)
  a.deepStrictEqual(output.unknown, [])
})

runner.test('output.setValue(value): multiple', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', multiple: true, defaultOption: true }
  ])
  const output = new Output(definitions)
  a.deepStrictEqual(output.get('one'), undefined)
  output.setValue('1')
  a.deepStrictEqual(output.get('one'), [ '1' ])
  output.setValue('2')
  a.deepStrictEqual(output.get('one'), [ '1', '2' ])
})
