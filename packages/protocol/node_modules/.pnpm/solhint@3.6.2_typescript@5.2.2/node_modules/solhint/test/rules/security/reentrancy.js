const linter = require('../../../lib/index')
const { assertWarnsCount, assertErrorMessage, assertNoWarnings } = require('../../common/asserts')

describe('Linter - reentrancy', () => {
  require('../../fixtures/security/reentrancy-vulenarble').forEach((curCode) =>
    it('should return warn when code contains possible reentrancy', () => {
      const report = linter.processStr(curCode, {
        rules: { reentrancy: 'warn' },
      })

      assertWarnsCount(report, 1)
      assertErrorMessage(report, 'reentrancy')
    })
  )

  require('../../fixtures/security/reentrancy-invulenarble').forEach((curCode) =>
    it('should not return warn when code do not contains transfer', () => {
      const report = linter.processStr(curCode, {
        rules: { reentrancy: 'warn' },
      })

      assertNoWarnings(report)
    })
  )
})
