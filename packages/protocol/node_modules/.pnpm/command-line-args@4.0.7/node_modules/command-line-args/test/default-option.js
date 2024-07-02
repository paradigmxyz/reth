'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('defaultOption: string', function () {
  const optionDefinitions = [
    { name: 'files', defaultOption: true }
  ]
  const argv = [ 'file1', 'file2' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    files: 'file1'
  })
})

runner.test('defaultOption: multiple string', function () {
  const optionDefinitions = [
    { name: 'files', defaultOption: true, multiple: true }
  ]
  const argv = [ 'file1', 'file2' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    files: [ 'file1', 'file2' ]
  })
})

runner.test('defaultOption: after a boolean', function () {
  const definitions = [
    { name: 'one', type: Boolean },
    { name: 'two', defaultOption: true }
  ]
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ '--one', 'sfsgf' ] }),
    { one: true, two: 'sfsgf' }
  )
})

runner.test('defaultOption: multiple defaultOption values between other arg/value pairs', function () {
  const optionDefinitions = [
    { name: 'one' },
    { name: 'two' },
    { name: 'files', defaultOption: true, multiple: true }
  ]
  const argv = [ '--one', '1', 'file1', 'file2', '--two', '2' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    one: '1',
    two: '2',
    files: [ 'file1', 'file2' ]
  })
})

runner.test('defaultOption: multiple defaultOption values between other arg/value pairs 2', function () {
  const optionDefinitions = [
    { name: 'one', type: Boolean },
    { name: 'two' },
    { name: 'files', defaultOption: true, multiple: true }
  ]
  const argv = [ 'file0', '--one', 'file1', '--files', 'file2', '--two', '2', 'file3' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    one: true,
    two: '2',
    files: [ 'file0', 'file1', 'file2', 'file3' ]
  })
})

runner.test('defaultOption: floating args present but no defaultOption', function () {
  const definitions = [
    { name: 'one', type: Boolean }
  ]
  a.deepStrictEqual(
    commandLineArgs(definitions, { argv: [ 'aaa', '--one', 'aaa', 'aaa' ] }),
    { one: true }
  )
})

runner.test('defaultOption: non-multiple should take first value', function () {
  const optionDefinitions = [
    { name: 'file', defaultOption: true }
  ]
  const argv = [ 'file1', 'file2' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    file: 'file1'
  })
})

runner.test('defaultOption: non-multiple should take first value 2', function () {
  const optionDefinitions = [
    { name: 'file', defaultOption: true },
    { name: 'one', type: Boolean },
    { name: 'two', type: Boolean }
  ]
  const argv = [ '--two', 'file1', '--one', 'file2' ]
  a.deepStrictEqual(commandLineArgs(optionDefinitions, { argv }), {
    file: 'file1',
    two: true,
    one: true
  })
})
