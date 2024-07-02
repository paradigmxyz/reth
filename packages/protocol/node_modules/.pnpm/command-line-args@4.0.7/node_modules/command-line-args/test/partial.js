'use strict'
const TestRunner = require('test-runner')
const commandLineArgs = require('../')
const a = require('assert')

const runner = new TestRunner()

runner.test('partial: simple', function () {
  const definitions = [
    { name: 'one', type: Boolean }
  ]
  const argv = [ '--two', 'two', '--one', 'two' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    one: true,
    _unknown: [ '--two', 'two', 'two' ]
  })
})

runner.test('partial: defaultOption', function () {
  const definitions = [
    { name: 'files', type: String, defaultOption: true, multiple: true }
  ]
  const argv = [ '--files', 'file1', '--one', 'file2' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file1', 'file2' ],
    _unknown: [ '--one' ]
  })
})

runner.test('partial: defaultOption 2', function () {
  const definitions = [
    { name: 'files', type: String, defaultOption: true, multiple: true },
    { name: 'one', type: Boolean },
    { name: 'two', alias: 't', defaultValue: 2 }
  ]
  const argv = [ 'file1', '--one', 'file2', '-t', '--two=3', 'file3', '-ab' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file1', 'file2', 'file3' ],
    two: '3',
    one: true,
    _unknown: [ '-a', '-b' ]
  })
})

runner.test('partial: defaultOption with value equal to defaultValue', function () {
  const definitions = [
    { name: 'file', type: String, defaultOption: true, defaultValue: 'file1' }
  ]
  const argv = [ 'file1', '--two=3', '--four', '5' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    file: 'file1',
    _unknown: [ '--two', '3', '--four', '5' ]
  })
})

runner.test('partial: defaultOption with value equal to defaultValue 2', function () {
  const definitions = [
    { name: 'file', type: String, defaultOption: true, defaultValue: 'file1' }
  ]
  const argv = [ '--file', '--file=file1', '--two=3', '--four', '5' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    file: 'file1',
    _unknown: [ '--two', '3', '--four', '5' ]
  })
})

runner.test('partial: multiple', function () {
  const definitions = [
    { name: 'files', type: String, multiple: true }
  ]
  const argv = [ 'file1', '--files', 'file2', '-t', '--two=3', 'file3', '-ab', '--files=file4' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file2', 'file4' ],
    _unknown: [ 'file1', '-t', '--two=3', 'file3', '-a', '-b' ]
  })
})

runner.test('unknown options: rejected defaultOption values end up in _unknown', function () {
  const definitions = [
    { name: 'foo', type: String },
    { name: 'verbose', alias: 'v', type: Boolean },
    { name: 'libs', type: String, defaultOption: true }
  ]
  const argv = [ '--foo', 'bar', '-v', 'libfn', '--libarg', 'val1', '-r' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    foo: 'bar',
    verbose: true,
    libs: 'libfn',
    _unknown: [ '--libarg', 'val1', '-r' ]
  })
})

runner.test('partial: groups', function () {
  const definitions = [
    { name: 'one', group: 'a' },
    { name: 'two', group: 'a' },
    { name: 'three', group: 'b' }
  ]
  const argv = [ '--one', '1', '--two', '2', '--three', '3', 'ham', '--cheese' ]
  a.deepStrictEqual(commandLineArgs(definitions, { argv, partial: true }), {
    a: {
      one: '1',
      two: '2'
    },
    b: {
      three: '3'
    },
    _all: {
      one: '1',
      two: '2',
      three: '3'
    },
    _unknown: [ 'ham', '--cheese' ]
  })
})

runner.test('partial: multiple groups and _none', function () {
  const definitions = [
    { name: 'one', group: ['a', 'f'] },
    { name: 'two', group: ['a', 'g'] },
    { name: 'three' }
  ]
  const argv = [ '--cheese', '--one', '1', 'ham', '--two', '2', '--three', '3', '-c' ]
  a.deepStrictEqual(commandLineArgs(definitions, { argv, partial: true }), {
    a: {
      one: '1',
      two: '2'
    },
    f: {
      one: '1'
    },
    g: {
      two: '2'
    },
    _none: {
      three: '3'
    },
    _all: {
      one: '1',
      two: '2',
      three: '3'
    },
    _unknown: [ '--cheese', 'ham', '-c' ]
  })
})

runner.test('partial: defaultOption with --option=value notation', function () {
  const definitions = [
    { name: 'files', type: String, multiple: true, defaultOption: true }
  ]
  const argv = [ 'file1', 'file2', '--unknown=something' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file1', 'file2' ],
    _unknown: [ '--unknown=something' ]
  })
})

runner.test('partial: defaultOption with --option=value notation 2', function () {
  const definitions = [
    { name: 'files', type: String, multiple: true, defaultOption: true }
  ]
  const argv = [ 'file1', 'file2', '--unknown=something', '--files', 'file3', '--files=file4' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file1', 'file2', 'file3', 'file4' ],
    _unknown: [ '--unknown=something' ]
  })
})

runner.test('partial: defaultOption with --option=value notation 3', function () {
  const definitions = [
    { name: 'files', type: String, multiple: true, defaultOption: true }
  ]
  const argv = [ '--unknown', 'file1', '--another', 'something', 'file2', '--unknown=something', '--files', 'file3', '--files=file4' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    files: [ 'file1', 'something', 'file2', 'file3', 'file4' ],
    _unknown: [ '--unknown', '--another', '--unknown=something' ]
  })
})

runner.test('partial: mulitple unknowns with same name', function () {
  const definitions = [
    { name: 'file' }
  ]
  const argv = [ '--unknown', '--unknown=something', '--file=file1', '--unknown' ]
  const options = commandLineArgs(definitions, { argv, partial: true })
  a.deepStrictEqual(options, {
    file: 'file1',
    _unknown: [ '--unknown', '--unknown=something', '--unknown' ]
  })
})
