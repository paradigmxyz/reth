const { assertErrorCount, assertNoErrors, assertErrorMessage } = require('../../common/asserts')
const linter = require('../../../lib/index')
const { contractWith, stateDef } = require('../../common/contract-builder')

describe('Linter - max-states-count', () => {
  it('should raise error when count of states too big', () => {
    const code = require('../../fixtures/best-practises/number-of-states-high')

    const report = linter.processStr(code, {
      rules: { 'max-states-count': ['error', 15] },
    })

    assertErrorCount(report, 1)
    assertErrorMessage(report, 'no more than 15')
  })

  it('should not raise error for count of states that lower that max', () => {
    const code = require('../../fixtures/best-practises/number-of-states-low')

    const report = linter.processStr(code, {
      rules: { 'max-states-count': 'error' },
    })

    assertNoErrors(report)
  })

  it('should not raise error for count of states when it value increased in config', () => {
    const code = contractWith(stateDef(20))

    const report = linter.processStr(code, { rules: { 'max-states-count': ['error', 20] } })

    assertNoErrors(report)
  })

  it('should only count state declarations', () => {
    const code = contractWith(`
      uint private a;
      uint private b;
      uint private c;

      function f() {}
    `)

    const report = linter.processStr(code, { rules: { 'max-states-count': ['error', 3] } })

    assertNoErrors(report)
  })

  it('should not count immutable variables', () => {
    const code = contractWith(`
      uint public immutable a;
      uint public b;
      uint public c;

      function f() {}
    `)

    const report = linter.processStr(code, { rules: { 'max-states-count': ['error', 2] } })

    assertNoErrors(report)
  })
})
