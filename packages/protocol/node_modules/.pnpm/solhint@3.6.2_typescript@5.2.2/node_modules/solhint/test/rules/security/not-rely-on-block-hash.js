const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertWarnsCount, assertErrorMessage } = require('../../common/asserts')

describe('Linter - not-rely-on-block-hash', () => {
  it('should return warn when function rely on block has', () => {
    const code = funcWith('end >= block.blockhash + daysAfter * 1 days;')

    const report = linter.processStr(code, {
      rules: { 'not-rely-on-block-hash': 'warn' },
    })

    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'block.blockhash')
  })
})
