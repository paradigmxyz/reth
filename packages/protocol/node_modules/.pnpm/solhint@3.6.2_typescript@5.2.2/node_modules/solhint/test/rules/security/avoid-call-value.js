const assert = require('assert')
const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith

describe('Linter - avoid-call-value', () => {
  it('should return "call.value" verification error', () => {
    const code = funcWith('x.call.value(55)();')

    const report = linter.processStr(code, {
      rules: { 'avoid-call-value': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.reports[0].message.includes('call.value'))
  })
})
