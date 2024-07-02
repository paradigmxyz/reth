'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('default value', function () {
  let defs = [
    { name: 'one' },
    { name: 'two', defaultValue: 'two' }
  ]
  let argv = [ '--one', '1' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    one: '1',
    two: 'two'
  })

  defs = [ { name: 'two', defaultValue: 'two' } ]
  argv = []
  a.deepStrictEqual(commandLineArgs(defs, { argv }), { two: 'two' })

  defs = [ { name: 'two', defaultValue: 'two' } ]
  argv = [ '--two', 'zwei' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), { two: 'zwei' })

  defs = [ { name: 'two', multiple: true, defaultValue: [ 'two', 'zwei' ] } ]
  argv = [ '--two', 'duo' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), { two: [ 'duo' ] })
})

runner.test('default value 2', function () {
  const defs = [{ name: 'two', multiple: true, defaultValue: ['two', 'zwei'] }]
  const result = commandLineArgs(defs, [])
  a.deepStrictEqual(result, { two: [ 'two', 'zwei' ] })
})

runner.test('default value: array as defaultOption', function () {
  const defs = [
    { name: 'two', multiple: true, defaultValue: ['two', 'zwei'], defaultOption: true }
  ]
  const argv = [ 'duo' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), { two: [ 'duo' ] })
})

runner.test('default value: falsy default values', function () {
  const defs = [
    { name: 'one', defaultValue: 0 },
    { name: 'two', defaultValue: false }
  ]

  const argv = []
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    one: 0,
    two: false
  })
})

runner.test('default value: is arrayifed if multiple set', function () {
  const defs = [
    { name: 'one', defaultValue: 0, multiple: true }
  ]

  let argv = []
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    one: [ 0 ]
  })
  argv = [ '--one', '2' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    one: [ '2' ]
  })
})

runner.test('default value: combined with defaultOption', function () {
  const defs = [
    { name: 'path', defaultOption: true, defaultValue: './' }
  ]

  let argv = [ '--path', 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: 'test'
  })
  argv = [ 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: 'test'
  })
  argv = [ ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: './'
  })
})

runner.test('default value: combined with multiple and defaultOption', function () {
  const defs = [
    { name: 'path', multiple: true, defaultOption: true, defaultValue: './' }
  ]

  let argv = [ '--path', 'test1', 'test2' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test1', 'test2' ]
  })
  argv = [ '--path', 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test' ]
  })
  argv = [ 'test1', 'test2' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test1', 'test2' ]
  })
  argv = [ 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test' ]
  })
  argv = [ ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ './' ]
  })
})

runner.test('default value: array default combined with multiple and defaultOption', function () {
  const defs = [
    { name: 'path', multiple: true, defaultOption: true, defaultValue: [ './' ] }
  ]

  let argv = [ '--path', 'test1', 'test2' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test1', 'test2' ]
  })
  argv = [ '--path', 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test' ]
  })
  argv = [ 'test1', 'test2' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test1', 'test2' ]
  })
  argv = [ 'test' ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ 'test' ]
  })
  argv = [ ]
  a.deepStrictEqual(commandLineArgs(defs, { argv }), {
    path: [ './' ]
  })
})
