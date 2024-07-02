const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('Linter - no-complex-fallback', () => {
  it('should return that fallback must be simple', () => {
    const code = contractWith(`function () public payable {
            make1();
            make2();
        }`)

    const report = linter.processStr(code, {
      rules: { 'no-complex-fallback': 'warn' },
    })

    assert.equal(report.warningCount, 1)
    assert.ok(report.reports[0].message.includes('Fallback'))
  })
})
