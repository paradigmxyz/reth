const assert = require('assert')
const linter = require('../../../lib/index')
const { contractWith } = require('../../common/contract-builder')

describe('Linter - visibility-modifier-order', () => {
  it('should raise visibility modifier error', () => {
    const code = require('../../fixtures/order/visibility-modifier-not-first')

    const report = linter.processStr(code, {
      rules: { 'visibility-modifier-order': 'error' },
    })

    assert.equal(report.errorCount, 1)
  })

  it('should not raise visibility modifier error', () => {
    const code = require('../../fixtures/order/visibility-modifier-first')

    const report = linter.processStr(code, {
      rules: { 'visibility-modifier-order': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not raise error if a payable address is a parameter', () => {
    const code = contractWith('function foo(address payable addr) public payable {}')

    const report = linter.processStr(code, {
      rules: { 'visibility-modifier-order': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })
})
