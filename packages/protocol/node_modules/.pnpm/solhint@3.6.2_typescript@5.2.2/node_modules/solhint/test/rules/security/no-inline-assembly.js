const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertWarnsCount, assertErrorMessage } = require('../../common/asserts')

describe('Linter - no-inline-assembly', () => {
  it('should return warn when function use inline assembly', () => {
    const code = funcWith(' assembly { "test" } ')

    const report = linter.processStr(code, {
      rules: { 'no-inline-assembly': 'warn' },
    })

    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'assembly')
  })
})
