const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const CONTRACTS = require('../../fixtures/security/contracts-with-free-functions')

describe('Linter - func-visibility with free functions', () => {
  it('should return two warnings and skip free functions', () => {
    const code = CONTRACTS.CONTRACTS_FREE_FUNCTIONS_ERRORS_2
    const report = linter.processStr(code, {
      rules: { 'func-visibility': ['warn', { ignoreConstructors: true }] },
    })

    assert.equal(report.warningCount, 2)
    assert.ok(report.reports[0].message.includes('visibility'))
    assert.ok(report.reports[1].message.includes('visibility'))
  })

  it('should return one warning and skip free functions', () => {
    const code = CONTRACTS.CONTRACT_FREE_FUNCTIONS_ERRORS_1
    const report = linter.processStr(code, {
      rules: { 'func-visibility': ['warn', { ignoreConstructors: true }] },
    })

    assert.equal(report.warningCount, 1)
    assert.ok(report.reports[0].message.includes('visibility'))
  })

  it('should not return any warning for a free function only', () => {
    const code = CONTRACTS.NOCONTRACT_FREE_FUNCTION_ERRORS_0
    const report = linter.processStr(code, {
      rules: { 'func-visibility': ['warn', { ignoreConstructors: true }] },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return any warning for a correct contract with a free function', () => {
    const code = CONTRACTS.CONTRACT_FREE_FUNCTIONS_ERRORS_0
    const report = linter.processStr(code, {
      rules: { 'func-visibility': ['warn', { ignoreConstructors: true }] },
    })

    assert.equal(report.warningCount, 0)
  })
})

describe('Linter - func-visibility', () => {
  it('should return required visibility error', () => {
    require('../../fixtures/security/functions-without-visibility').forEach((func) => {
      const code = contractWith(func)

      const report = linter.processStr(code, {
        rules: { 'func-visibility': 'warn' },
      })

      assert.equal(report.warningCount, 1)
      assert.ok(report.reports[0].message.includes('visibility'))
    })
  })

  it('should not return required visibility error', () => {
    require('../../fixtures/security/functions-with-visibility').forEach((func) => {
      const code = contractWith(func)

      const report = linter.processStr(code, {
        rules: { 'func-visibility': 'warn' },
      })

      assert.equal(report.warningCount, 0)
    })
  })

  describe("when 'ignoreConstructors' is enabled", () => {
    it('should ignore constructors without visibility', () => {
      const code = contractWith('constructor () {}')

      const report = linter.processStr(code, {
        rules: {
          'func-visibility': ['warn', { ignoreConstructors: true }],
        },
      })

      assert.equal(report.warningCount, 0)
    })

    it('should still report functions without visibility', () => {
      const code = contractWith('function foo() {}')

      const report = linter.processStr(code, {
        rules: {
          'func-visibility': ['warn', { ignoreConstructors: true }],
        },
      })

      assert.equal(report.warningCount, 1)
    })
  })
})
