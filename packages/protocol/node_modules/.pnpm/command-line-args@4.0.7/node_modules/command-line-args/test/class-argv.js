'use strict'
const TestRunner = require('test-runner')
const a = require('assert')
const Argv = require('../lib/argv')
const Definitions = require('../lib/definitions')

const runner = new TestRunner()

runner.test('.expandOptionEqualsNotation()', function () {
  const argv = new Argv()
  argv.load([ '--one=1', '--two', '2', '--three=3', '4' ])
  argv.expandOptionEqualsNotation()
  a.deepEqual(argv, [
    '--one', '552f3a31-14cd-4ced-bd67-656a659e9efb1', '--two', '2', '--three', '552f3a31-14cd-4ced-bd67-656a659e9efb3', '4'
  ])
})

runner.test('.expandGetoptNotation()', function () {
  const argv = new Argv()
  argv.load([ '-abc' ])
  argv.expandGetoptNotation()
  a.deepEqual(argv.slice(), [
    '-a', '-b', '-c'
  ])
})

runner.test('.expandGetoptNotation() with values', function () {
  const argv = new Argv()
  argv.load([ '-abc', '1', '-a', '2', '-bc' ])
  argv.expandGetoptNotation()
  a.deepEqual(argv, [
    '-a', '-b', '-c', '1', '-a', '2', '-b', '-c'
  ])
})

runner.test('.validate()', function () {
  const definitions = new Definitions()
  definitions.load([
    { name: 'one', type: Number }
  ])

  a.doesNotThrow(function () {
    const argv = new Argv()
    argv.load([ '--one', '1' ])
    argv.validate(definitions)
  })

  a.throws(function () {
    const argv = new Argv()
    argv.load([ '--one', '--two' ])
    argv.validate(definitions)
  })

  a.throws(function () {
    const argv = new Argv()
    argv.load([ '--one', '2', '--two', 'two' ])
    argv.validate(definitions)
  })

  a.throws(function () {
    const argv = new Argv()
    argv.load([ '-a', '2' ])
    argv.validate(definitions)
  })
})

runner.test('expandOptionEqualsNotation', function () {
  const argv = new Argv()
  argv.load([ '--one=tree' ])
  argv.expandOptionEqualsNotation()
  a.deepEqual(argv, [ '--one', '552f3a31-14cd-4ced-bd67-656a659e9efbtree' ])
})
