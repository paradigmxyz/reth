const assert = require('assert')
const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith
const { assertErrorMessage, assertErrorCount } = require('../../common/asserts')

describe('Linter - multiple-sends', () => {
  it('should return the error that multiple send calls are being used in the same transaction', () => {
    const code = funcWith(`
          uint aRes = a.send(1);
          uint bRes = b.send(2);
        `)

    const report = linter.processStr(code, {
      rules: { 'multiple-sends': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.reports[0].message.includes('multiple'))
  })

  it('should return the error that multiple send calls are being used inside a loop', () => {
    const code = funcWith(`
          while (ac > b) { uint res = a.send(1); }
        `)

    const report = linter.processStr(code, {
      rules: { 'multiple-sends': 'error' },
    })

    assertErrorCount(report, 1)
    assertErrorMessage(report, 'multiple')
  })
})
