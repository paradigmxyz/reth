'use strict'
const TestRunner = require('test-runner')
const a = require('assert')
const option = require('../lib/option')

const runner = new TestRunner()

runner.test('option', function () {
  a.strictEqual(option.isOption('--yeah'), true)
  a.strictEqual(option.isOption('в--yeah'), false)
})
