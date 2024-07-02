const assert = require('assert')
const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertErrorMessage } = require('../../common/asserts')

describe('Linter - avoid-tx-origin', () => {
  it('should return error that used tx.origin', () => {
    const code = funcWith(`
          uint aRes = tx.origin;
        `)

    const report = linter.processStr(code, {
      rules: { 'avoid-tx-origin': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assertErrorMessage(report, 'origin')
  })
})
