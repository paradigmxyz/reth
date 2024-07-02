const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertWarnsCount, assertErrorMessage } = require('../../common/asserts')

describe('Linter - not-rely-on-time', () => {
  const TIME_BASED_LOGIC = [
    funcWith('now >= start + daysAfter * 1 days;'),
    funcWith('start >= block.timestamp + daysAfter * 1 days;'),
  ]

  TIME_BASED_LOGIC.forEach((curCode) =>
    it('should return warn when business logic rely on time', () => {
      const report = linter.processStr(curCode, {
        rules: { 'not-rely-on-time': 'warn' },
      })

      assertWarnsCount(report, 1)
      assertErrorMessage(report, 'time')
    })
  )
})
