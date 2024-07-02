const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertWarnsCount, assertErrorMessage } = require('../../common/asserts')

describe('Linter - avoid-low-level-calls', () => {
  const LOW_LEVEL_CALLS = require('../../fixtures/security/low-level-calls')
  const WARN_LOW_LEVEL_CALLS = LOW_LEVEL_CALLS[0].map(funcWith)
  const ALLOWED_LOW_LEVEL_CALLS = LOW_LEVEL_CALLS[1].map(funcWith)

  WARN_LOW_LEVEL_CALLS.forEach((curCode) =>
    it('should return warn when code contains low level calls', () => {
      const report = linter.processStr(curCode, {
        rules: { 'avoid-low-level-calls': 'warn' },
      })

      assertWarnsCount(report, 1)
      assertErrorMessage(report, 'low level')
    })
  )

  ALLOWED_LOW_LEVEL_CALLS.forEach((curCode) =>
    it('should not return warn when code contains allowed low level calls', () => {
      const report = linter.processStr(curCode, {
        rules: { 'avoid-low-level-calls': 'warn' },
      })

      assertWarnsCount(report, 0)
    })
  )
})
